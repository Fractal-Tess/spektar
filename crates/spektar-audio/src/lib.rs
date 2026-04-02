use anyhow::{anyhow, bail, Context as AnyhowContext, Result};
use psimple::Simple;
use pulse::{
    context::{Context as PulseContext, FlagSet as PulseContextFlags, State as PulseContextState},
    def::BufferAttr,
    mainloop::standard::{IterateResult, Mainloop as PulseMainloop},
    sample::{Format as PulseFormat, Spec as PulseSpec},
    stream::Direction as PulseDirection,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const APP_TITLE: &str = "Spektar";
pub const MAX_BUFFERED_SAMPLES: usize = 2048 * 16;
pub const READ_FRAMES: usize = 256;

pub type SharedSampleBuffer = Arc<Mutex<VecDeque<f32>>>;
pub type SharedSampleRate = Arc<Mutex<u32>>;
pub type SharedAudioStatus = Arc<Mutex<AudioStatus>>;
pub type CaptureStopFlag = Arc<AtomicBool>;

#[derive(Clone, Default)]
pub struct AudioStatus {
    pub backend: String,
    pub source: String,
    pub message: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct AudioConfig {
    pub source_name: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub backend_name: String,
}

#[derive(Clone)]
pub struct DiagnosticConfig {
    enabled: bool,
    started_at: Instant,
    duration: Duration,
}

impl DiagnosticConfig {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            started_at: Instant::now(),
            duration: Duration::from_secs(5),
        }
    }

    pub fn active(&self) -> bool {
        self.enabled && self.started_at.elapsed() <= self.duration
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }
}

pub struct SampleWindow {
    pub samples: Option<Vec<f32>>,
    pub buffer_len_before: usize,
    pub drained_samples: usize,
    pub lock_wait: Duration,
}

struct PushStats {
    samples_added: usize,
    trimmed: usize,
    buffer_len_after: usize,
}

pub fn initial_audio_status() -> AudioStatus {
    AudioStatus {
        backend: "PulseAudio monitor capture".to_owned(),
        source: "resolving default sink".to_owned(),
        message: "Connecting to the system output monitor…".to_owned(),
        error: None,
    }
}

pub fn new_sample_buffer() -> SharedSampleBuffer {
    Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFERED_SAMPLES)))
}

pub fn new_sample_rate(default_rate: u32) -> SharedSampleRate {
    Arc::new(Mutex::new(default_rate))
}

pub fn new_audio_status() -> SharedAudioStatus {
    Arc::new(Mutex::new(initial_audio_status()))
}

pub fn new_capture_stop() -> CaptureStopFlag {
    Arc::new(AtomicBool::new(false))
}

pub fn latest_sample_window(sample_buffer: &SharedSampleBuffer, fft_size: usize) -> SampleWindow {
    let lock_started = Instant::now();

    if let Ok(mut buffer) = sample_buffer.lock() {
        let buffer_len_before = buffer.len();
        if buffer_len_before < fft_size {
            return SampleWindow {
                samples: None,
                buffer_len_before,
                drained_samples: 0,
                lock_wait: lock_started.elapsed(),
            };
        }

        while buffer.len() > fft_size {
            buffer.pop_front();
        }

        let drained_samples = buffer_len_before.saturating_sub(fft_size);
        let samples = buffer.iter().copied().collect();
        return SampleWindow {
            samples: Some(samples),
            buffer_len_before,
            drained_samples,
            lock_wait: lock_started.elapsed(),
        };
    }

    SampleWindow {
        samples: None,
        buffer_len_before: 0,
        drained_samples: 0,
        lock_wait: lock_started.elapsed(),
    }
}

pub fn spawn_audio_capture(
    sample_buffer: SharedSampleBuffer,
    sample_rate: SharedSampleRate,
    audio_status: SharedAudioStatus,
    stop: CaptureStopFlag,
    requested_sample_rate: u32,
    diagnostic: Arc<DiagnosticConfig>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut config = resolve_default_sink_monitor().context(
                "failed to resolve the default sink monitor via PulseAudio (matching Cava's auto-source logic)",
            )?;
            config.sample_rate = requested_sample_rate;

            set_status(
                &audio_status,
                AudioStatus {
                    backend: config.backend_name.clone(),
                    source: config.source_name.clone(),
                    message: "Recording the active output monitor…".to_owned(),
                    error: None,
                },
            );

            if let Ok(mut rate) = sample_rate.lock() {
                *rate = config.sample_rate;
            }

            capture_monitor_stream(
                config,
                sample_buffer,
                audio_status.clone(),
                stop,
                diagnostic,
            )
        })();

        if let Err(error) = result {
            let mut status = audio_status.lock().expect("audio status lock poisoned");
            status.error = Some(error.to_string());
            if status.message.is_empty() {
                status.message = "Audio capture did not start".to_owned();
            }
        }
    })
}

fn resolve_default_sink_monitor() -> Result<AudioConfig> {
    let mut mainloop =
        PulseMainloop::new().ok_or_else(|| anyhow!("failed to create PulseAudio mainloop"))?;
    let mut context = PulseContext::new(&mainloop, APP_TITLE)
        .ok_or_else(|| anyhow!("failed to create PulseAudio context"))?;

    context
        .connect(None, PulseContextFlags::NOFLAGS, None)
        .context("failed to connect to the PulseAudio server")?;

    loop {
        match mainloop.iterate(true) {
            IterateResult::Success(_) => match context.get_state() {
                PulseContextState::Ready => break,
                PulseContextState::Failed | PulseContextState::Terminated => {
                    bail!("PulseAudio context failed before it became ready")
                }
                _ => {}
            },
            IterateResult::Err(error) => {
                return Err(anyhow!("PulseAudio mainloop error: {error:?}"));
            }
            IterateResult::Quit(_) => {
                bail!("PulseAudio mainloop exited before server info was available")
            }
        }
    }

    let server_info = Arc::new(Mutex::new(None::<AudioConfig>));
    let finished = Arc::new(AtomicBool::new(false));

    let server_info_ref = Arc::clone(&server_info);
    let finished_ref = Arc::clone(&finished);
    let introspector = context.introspect();
    let _operation = introspector.get_server_info(move |info| {
        let sink_monitor = info
            .default_sink_name
            .as_deref()
            .map(|name| format!("{name}.monitor"))
            .or_else(|| info.default_source_name.as_deref().map(str::to_owned));

        if let Some(source_name) = sink_monitor {
            *server_info_ref.lock().expect("server info lock poisoned") = Some(AudioConfig {
                source_name,
                sample_rate: info.sample_spec.rate.max(8_000),
                channels: info.sample_spec.channels.max(1),
                backend_name: format!(
                    "{} monitor capture",
                    info.server_name.as_deref().unwrap_or("PulseAudio")
                ),
            });
        }

        finished_ref.store(true, AtomicOrdering::Relaxed);
    });

    while !finished.load(AtomicOrdering::Relaxed) {
        match mainloop.iterate(true) {
            IterateResult::Success(_) => {}
            IterateResult::Err(error) => {
                return Err(anyhow!("PulseAudio server-info query failed: {error:?}"));
            }
            IterateResult::Quit(_) => {
                bail!("PulseAudio mainloop quit while resolving the default sink monitor")
            }
        }
    }

    context.disconnect();

    let resolved = {
        let server_info = server_info.lock().expect("server info lock poisoned");
        server_info.clone()
    };

    Ok(resolved.ok_or_else(|| anyhow!("PulseAudio did not provide a default sink/source name"))?)
}

fn capture_monitor_stream(
    config: AudioConfig,
    sample_buffer: SharedSampleBuffer,
    audio_status: SharedAudioStatus,
    stop: CaptureStopFlag,
    diagnostic: Arc<DiagnosticConfig>,
) -> Result<()> {
    let capture_spec = PulseSpec {
        format: PulseFormat::S16NE,
        rate: config.sample_rate,
        channels: config.channels.max(1),
    };

    if !capture_spec.is_valid() {
        bail!(
            "invalid PulseAudio capture spec (rate={}, channels={})",
            capture_spec.rate,
            capture_spec.channels
        );
    }

    let buffer_attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: (READ_FRAMES * capture_spec.channels as usize * 2) as u32,
    };

    let stream = Simple::new(
        None,
        APP_TITLE,
        PulseDirection::Record,
        Some(config.source_name.as_str()),
        "spektar-monitor-capture",
        &capture_spec,
        None,
        Some(&buffer_attr),
    )
    .with_context(|| format!("failed to open PulseAudio source {}", config.source_name))?;

    let mut raw = vec![0_u8; READ_FRAMES * capture_spec.channels as usize * 2];
    let mut read_index: u64 = 0;
    set_status(
        &audio_status,
        AudioStatus {
            backend: config.backend_name,
            source: config.source_name,
            message: format!(
                "Streaming {} Hz / {} channel monitor audio",
                capture_spec.rate, capture_spec.channels
            ),
            error: None,
        },
    );

    while !stop.load(AtomicOrdering::Relaxed) {
        let read_started = Instant::now();
        stream
            .read(&mut raw)
            .context("failed while reading monitor samples from PulseAudio")?;
        let read_duration = read_started.elapsed();
        let push_stats = push_samples(&sample_buffer, &raw, capture_spec.channels as usize);
        read_index += 1;

        if diagnostic.active() {
            eprintln!(
                "[diag][audio] t={:.3}s read_idx={} read_ms={:.2} bytes={} added={} trimmed={} buf_after={} rate={} channels={}",
                diagnostic.elapsed().as_secs_f64(),
                read_index,
                read_duration.as_secs_f64() * 1000.0,
                raw.len(),
                push_stats.samples_added,
                push_stats.trimmed,
                push_stats.buffer_len_after,
                capture_spec.rate,
                capture_spec.channels
            );
        }
    }

    Ok(())
}

fn push_samples(sample_buffer: &SharedSampleBuffer, raw: &[u8], channels: usize) -> PushStats {
    let channel_count = channels.max(1);
    let mut buffer = sample_buffer.lock().expect("sample buffer lock poisoned");
    let mut added = 0usize;

    for frame in raw.chunks_exact(channel_count * 2) {
        let mut combined = 0.0_f32;
        for channel in 0..channel_count {
            let offset = channel * 2;
            let sample = i16::from_ne_bytes([frame[offset], frame[offset + 1]]);
            combined += sample as f32 / i16::MAX as f32;
        }

        buffer.push_back(combined / channel_count as f32);
        added += 1;
    }

    let mut trimmed = 0usize;
    while buffer.len() > MAX_BUFFERED_SAMPLES {
        buffer.pop_front();
        trimmed += 1;
    }

    PushStats {
        samples_added: added,
        trimmed,
        buffer_len_after: buffer.len(),
    }
}

fn set_status(audio_status: &SharedAudioStatus, next: AudioStatus) {
    *audio_status.lock().expect("audio status lock poisoned") = next;
}
