use anyhow::{anyhow, bail, Context as AnyhowContext, Result};
use eframe::{
    egui::{self, Color32, Pos2, Rect, Stroke, Vec2},
    NativeOptions, Renderer,
};
use psimple::Simple;
use pulse::{
    context::{Context as PulseContext, FlagSet as PulseContextFlags, State as PulseContextState},
    def::BufferAttr,
    mainloop::standard::{IterateResult, Mainloop as PulseMainloop},
    sample::{Format as PulseFormat, Spec as PulseSpec},
    stream::Direction as PulseDirection,
};
use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};
use std::{
    cmp::Ordering,
    collections::VecDeque,
    env,
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const APP_TITLE: &str = "Spektar";
const NUM_BANDS: usize = 40;
const FFT_SIZE: usize = 2048;
const MAX_BUFFERED_SAMPLES: usize = FFT_SIZE * 16;
const READ_FRAMES: usize = 256;
const CAVA_EQ_SCALE: f32 = 1.0 / 4096.0;

#[derive(Clone, Default)]
struct AudioStatus {
    backend: String,
    source: String,
    message: String,
    error: Option<String>,
}

#[derive(Clone)]
struct AudioConfig {
    source_name: String,
    sample_rate: u32,
    channels: u8,
    backend_name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponsePreset {
    Balanced,
    Monstercat,
    Punchy,
    Smooth,
}

impl ResponsePreset {
    const ALL: [Self; 4] = [Self::Balanced, Self::Monstercat, Self::Punchy, Self::Smooth];

    fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Monstercat => "Monstercat",
            Self::Punchy => "Punchy",
            Self::Smooth => "Smooth",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Balanced => "general-purpose Cava-like response",
            Self::Monstercat => "stronger low-end and smoother desktop-style decay",
            Self::Punchy => "faster hits with less lingering memory",
            Self::Smooth => "slower, softer response with gentle decay",
        }
    }

    fn tuning(self) -> ResponseTuning {
        match self {
            Self::Balanced => ResponseTuning {
                low_cutoff_hz: 50.0,
                high_cutoff_hz: 10_000.0,
                eq_power: 0.85,
                attack: 0.72,
                release: 0.32,
                bar_ceiling: 1.0,
                sensitivity_rise: 0.018,
                sensitivity_fall: 0.030,
                initial_sensitivity: 14.0,
            },
            Self::Monstercat => ResponseTuning {
                low_cutoff_hz: 50.0,
                high_cutoff_hz: 10_000.0,
                eq_power: 0.90,
                attack: 0.64,
                release: 0.24,
                bar_ceiling: 1.0,
                sensitivity_rise: 0.020,
                sensitivity_fall: 0.032,
                initial_sensitivity: 15.0,
            },
            Self::Punchy => ResponseTuning {
                low_cutoff_hz: 40.0,
                high_cutoff_hz: 12_000.0,
                eq_power: 0.82,
                attack: 0.85,
                release: 0.46,
                bar_ceiling: 1.0,
                sensitivity_rise: 0.016,
                sensitivity_fall: 0.030,
                initial_sensitivity: 13.0,
            },
            Self::Smooth => ResponseTuning {
                low_cutoff_hz: 60.0,
                high_cutoff_hz: 9_000.0,
                eq_power: 0.82,
                attack: 0.44,
                release: 0.16,
                bar_ceiling: 1.0,
                sensitivity_rise: 0.016,
                sensitivity_fall: 0.026,
                initial_sensitivity: 14.0,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct ResponseTuning {
    low_cutoff_hz: f32,
    high_cutoff_hz: f32,
    eq_power: f32,
    attack: f32,
    release: f32,
    bar_ceiling: f32,
    sensitivity_rise: f32,
    sensitivity_fall: f32,
    initial_sensitivity: f32,
}

struct SpectrumApp {
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
    audio_status: Arc<Mutex<AudioStatus>>,
    sample_rate: Arc<Mutex<u32>>,
    preset: ResponsePreset,
    current_bars: Vec<f32>,
    sensitivity: f32,
    lerp_smoothing: f32,
    target_fps: u32,
    analysis_hz: u32,
    debug_logging: bool,
    diagnostic_mode: bool,
    started_at: Instant,
    last_frame_at: Instant,
    last_analysis_at: Instant,
    debug_frame: u64,
    capture_stop: Arc<AtomicBool>,
    capture_thread: Option<thread::JoinHandle<()>>,
}

struct AudioProcessStats {
    buffer_len_before: usize,
    lock_wait: Duration,
    drained_samples: usize,
    fft_duration: Duration,
    raw_max: f32,
    had_fft: bool,
}

impl AudioProcessStats {
    fn skipped(buffer_len_before: usize, lock_wait: Duration) -> Self {
        Self {
            buffer_len_before,
            lock_wait,
            drained_samples: 0,
            fft_duration: Duration::ZERO,
            raw_max: 0.0,
            had_fft: false,
        }
    }
}

#[derive(Clone)]
struct DiagnosticConfig {
    enabled: bool,
    started_at: Instant,
    duration: Duration,
}

impl DiagnosticConfig {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            started_at: Instant::now(),
            duration: Duration::from_secs(5),
        }
    }

    fn active(&self) -> bool {
        self.enabled && self.started_at.elapsed() <= self.duration
    }

    fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

struct PushStats {
    samples_added: usize,
    trimmed: usize,
    buffer_len_after: usize,
}

impl SpectrumApp {
    fn new() -> Self {
        let audio_status = Arc::new(Mutex::new(AudioStatus {
            backend: "PulseAudio monitor capture".to_owned(),
            source: "resolving default sink".to_owned(),
            message: "Connecting to the system output monitor…".to_owned(),
            error: None,
        }));

        let sample_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_BUFFERED_SAMPLES)));
        let sample_rate = Arc::new(Mutex::new(44_100));
        let capture_stop = Arc::new(AtomicBool::new(false));
        let preset = ResponsePreset::Balanced;
        let tuning = preset.tuning();
        let debug_logging = env::var_os("SPEKTAR_DEBUG").is_some();
        let diagnostic_mode = env::var_os("SPEKTAR_DIAGNOSTIC").is_some();
        let diagnostic = Arc::new(DiagnosticConfig::new(diagnostic_mode));
        let started_at = diagnostic.started_at;
        let capture_thread = spawn_audio_capture(
            Arc::clone(&sample_buffer),
            Arc::clone(&sample_rate),
            Arc::clone(&audio_status),
            Arc::clone(&capture_stop),
            44_100,
            Arc::clone(&diagnostic),
        );

        Self {
            sample_buffer,
            audio_status,
            sample_rate,
            preset,
            current_bars: vec![0.0; NUM_BANDS],
            sensitivity: tuning.initial_sensitivity,
            lerp_smoothing: 0.12,
            target_fps: 60,
            analysis_hz: 60,
            debug_logging,
            diagnostic_mode,
            started_at,
            last_frame_at: started_at,
            last_analysis_at: started_at,
            debug_frame: 0,
            capture_stop,
            capture_thread: Some(capture_thread),
        }
    }

    fn reset_response_state(&mut self) {
        let tuning = self.preset.tuning();
        self.current_bars.fill(0.0);
        self.sensitivity = tuning.initial_sensitivity;
    }

    fn process_audio(&mut self) -> AudioProcessStats {
        let mut samples = Vec::with_capacity(FFT_SIZE);
        let lock_started = Instant::now();
        let buffer_len_before: usize;

        if let Ok(mut buffer) = self.sample_buffer.lock() {
            buffer_len_before = buffer.len();
            if buffer.len() < FFT_SIZE {
                return AudioProcessStats::skipped(buffer_len_before, lock_started.elapsed());
            }

            while buffer.len() > FFT_SIZE {
                buffer.pop_front();
            }

            samples.extend(buffer.iter().copied());
        } else {
            return AudioProcessStats::skipped(0, lock_started.elapsed());
        }

        let lock_wait = lock_started.elapsed();

        if samples.len() != FFT_SIZE {
            return AudioProcessStats::skipped(buffer_len_before, lock_wait);
        }

        let tuning = self.preset.tuning();
        let sample_rate = self.sample_rate.lock().map(|rate| *rate).unwrap_or(44_100);
        let hann_window = spectrum_analyzer::windows::hann_window(&samples);
        let fft_started = Instant::now();
        let spectrum_result = samples_fft_to_spectrum(
            &hann_window,
            sample_rate,
            FrequencyLimit::Range(tuning.low_cutoff_hz, tuning.high_cutoff_hz),
            None,
        );

        if let Ok(spectrum) = spectrum_result {
            let magnitudes: Vec<(f32, f32)> = spectrum
                .data()
                .iter()
                .map(|(frequency, magnitude)| (frequency.val(), magnitude.val()))
                .collect();

            let raw_bars = convert_spectrum_to_bands(&magnitudes, NUM_BANDS, tuning);
            let raw_max = raw_bars.iter().copied().fold(0.0_f32, f32::max);
            self.apply_cava_response(&raw_bars, tuning);
            return AudioProcessStats {
                buffer_len_before,
                lock_wait,
                drained_samples: buffer_len_before.saturating_sub(FFT_SIZE),
                fft_duration: fft_started.elapsed(),
                raw_max,
                had_fft: true,
            };
        }

        AudioProcessStats {
            buffer_len_before,
            lock_wait,
            drained_samples: buffer_len_before.saturating_sub(FFT_SIZE),
            fft_duration: fft_started.elapsed(),
            raw_max: 0.0,
            had_fft: false,
        }
    }

    fn apply_cava_response(&mut self, raw_bars: &[f32], tuning: ResponseTuning) {
        self.debug_frame += 1;
        let silence = raw_bars.iter().all(|value| *value < 0.000001);
        let overshoot = raw_bars
            .iter()
            .any(|value| (*value * self.sensitivity) > tuning.bar_ceiling);

        if overshoot {
            self.sensitivity *= 1.0 - tuning.sensitivity_fall;
        } else if !silence {
            self.sensitivity *= 1.0 + tuning.sensitivity_rise;
        }
        self.sensitivity = self.sensitivity.clamp(1.0, 64.0);

        for (index, raw_value) in raw_bars.iter().enumerate() {
            let target = (*raw_value * self.sensitivity).clamp(0.0, tuning.bar_ceiling);
            let current = self.current_bars[index];
            let blend = if target >= current {
                tuning.attack
            } else {
                tuning.release
            };
            let effective_blend = blend * (1.0 - self.lerp_smoothing) + 0.08 * self.lerp_smoothing;
            let next = current + (target - current) * effective_blend;
            self.current_bars[index] = next.clamp(0.0, tuning.bar_ceiling);
        }

        if self.debug_logging && self.debug_frame % 60 == 0 {
            let raw_max = raw_bars.iter().copied().fold(0.0_f32, f32::max);
            let bar_max = self.current_bars.iter().copied().fold(0.0_f32, f32::max);
            let first_five: Vec<String> = self
                .current_bars
                .iter()
                .take(5)
                .map(|value| format!("{value:.3}"))
                .collect();
            eprintln!(
                "[spektar] preset={} sens={:.3} raw_max={:.6} bar_max={:.3} first5=[{}]",
                self.preset.label(),
                self.sensitivity,
                raw_max,
                bar_max,
                first_five.join(", ")
            );
        }
    }

    fn draw_visualizer(&self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 18.0, Color32::BLACK);
        painter.rect_stroke(
            rect,
            18.0,
            Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 255, 255, 24)),
            egui::StrokeKind::Outside,
        );

        if self.current_bars.iter().all(|value| *value <= 0.0001) {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Waiting for default sink audio…",
                egui::FontId::proportional(22.0),
                Color32::from_gray(230),
            );
            return;
        }

        let bars_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 28.0, rect.top() + rect.height() * 0.16),
            Pos2::new(rect.right() - 28.0, rect.bottom() - 22.0),
        );

        self.draw_bars(&painter, bars_rect);
        painter.text(
            Pos2::new(rect.left() + 28.0, rect.top() + 22.0),
            egui::Align2::LEFT_TOP,
            format!(
                "DEFAULT SINK VISUALIZER • {}",
                self.preset.label().to_uppercase()
            ),
            egui::FontId::monospace(16.0),
            Color32::from_rgb(220, 228, 238),
        );
    }

    fn draw_bars(&self, painter: &egui::Painter, rect: Rect) {
        let band_width = rect.width() / NUM_BANDS as f32;

        for (index, value) in self.current_bars.iter().enumerate() {
            let height = value * rect.height() * 0.96;
            let x = rect.left() + index as f32 * band_width;
            let bar_rect = Rect::from_min_max(
                Pos2::new(x + band_width * 0.18, rect.bottom() - height),
                Pos2::new(x + band_width * 0.82, rect.bottom()),
            );
            painter.rect_filled(bar_rect, 4.0, Color32::from_rgb(197, 185, 255));
        }
    }

    fn draw_preset_controls(&mut self, ui: &mut egui::Ui) {
        let mut preset_changed = false;

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Response").strong());
            for preset in ResponsePreset::ALL {
                let selected = self.preset == preset;
                if ui.selectable_label(selected, preset.label()).clicked() {
                    self.preset = preset;
                    preset_changed = true;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(&mut self.lerp_smoothing, 0.0..=0.85).text("Lerp"));
            ui.add(egui::Slider::new(&mut self.target_fps, 15..=144).text("FPS"));
            ui.add(egui::Slider::new(&mut self.analysis_hz, 10..=120).text("Sampling"));
        });
        ui.small(self.preset.description());

        if preset_changed {
            self.reset_response_state();
        }
    }
}

impl eframe::App for SpectrumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let frame_delta = now.duration_since(self.last_frame_at);
        self.last_frame_at = now;
        let analysis_interval = Duration::from_secs_f32(1.0 / self.analysis_hz as f32);
        let should_analyze =
            self.diagnostic_mode || now.duration_since(self.last_analysis_at) >= analysis_interval;
        let stats = if should_analyze {
            self.last_analysis_at = now;
            self.process_audio()
        } else {
            AudioProcessStats::skipped(0, Duration::ZERO)
        };

        if self.diagnostic_mode {
            eprintln!(
                "[diag][frame] t={:.3}s dt_ms={:.2} fft={} buf_before={} lock_us={} drained={} fft_ms={:.2} raw_max={:.6} sens={:.3} bar_max={:.3}",
                self.started_at.elapsed().as_secs_f64(),
                frame_delta.as_secs_f64() * 1000.0,
                stats.had_fft,
                stats.buffer_len_before,
                stats.lock_wait.as_micros(),
                stats.drained_samples,
                stats.fft_duration.as_secs_f64() * 1000.0,
                stats.raw_max,
                self.sensitivity,
                self.current_bars.iter().copied().fold(0.0_f32, f32::max)
            );
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(Color32::BLACK)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                let available = ui.available_rect_before_wrap();
                let visualizer_rect = Rect::from_min_max(
                    available.min,
                    Pos2::new(available.max.x, available.max.y - 118.0),
                );

                self.draw_visualizer(ui, visualizer_rect);

                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(Rect::from_min_max(
                        Pos2::new(available.left(), available.bottom() - 102.0),
                        available.max,
                    )),
                    |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Spektar");
                            self.draw_preset_controls(ui);
                            if let Ok(status) = self.audio_status.lock() {
                                ui.label(format!("{} • {}", status.backend, status.source));
                                if let Some(error) = &status.error {
                                    ui.colored_label(Color32::from_rgb(255, 120, 120), error);
                                } else {
                                    ui.label(&status.message);
                                }
                            }
                            ui.small(
                                "Black background, Cava-style sink capture, and Cava-inspired bar response.",
                            );
                        });
                    },
                );
            });

        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / self.target_fps as f32));

        if self.diagnostic_mode && self.started_at.elapsed() >= Duration::from_secs(5) {
            eprintln!(
                "[diag][summary] elapsed={:.3}s bar_max={:.3} first10=[{}]",
                self.started_at.elapsed().as_secs_f64(),
                self.current_bars.iter().copied().fold(0.0_f32, f32::max),
                self.current_bars
                    .iter()
                    .take(10)
                    .map(|value| format!("{value:.3}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            self.capture_stop.store(true, AtomicOrdering::Relaxed);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl Drop for SpectrumApp {
    fn drop(&mut self) {
        self.capture_stop.store(true, AtomicOrdering::Relaxed);
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_audio_capture(
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: Arc<Mutex<u32>>,
    audio_status: Arc<Mutex<AudioStatus>>,
    stop: Arc<AtomicBool>,
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

    let resolved = server_info
        .lock()
        .expect("server info lock poisoned")
        .clone()
        .ok_or_else(|| anyhow!("PulseAudio did not provide a default sink/source name"));

    resolved
}

fn capture_monitor_stream(
    config: AudioConfig,
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
    audio_status: Arc<Mutex<AudioStatus>>,
    stop: Arc<AtomicBool>,
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

fn push_samples(
    sample_buffer: &Arc<Mutex<VecDeque<f32>>>,
    raw: &[u8],
    channels: usize,
) -> PushStats {
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

fn set_status(audio_status: &Arc<Mutex<AudioStatus>>, next: AudioStatus) {
    *audio_status.lock().expect("audio status lock poisoned") = next;
}

fn convert_spectrum_to_bands(
    spectrum: &[(f32, f32)],
    num_bands: usize,
    tuning: ResponseTuning,
) -> Vec<f32> {
    if spectrum.is_empty() {
        return vec![0.0; num_bands];
    }

    let low = tuning.low_cutoff_hz.max(1.0);
    let high = tuning.high_cutoff_hz.max(low + 1.0);
    let log_ratio = high / low;
    let mut bars = vec![0.0; num_bands];

    for (band_index, band) in bars.iter_mut().enumerate() {
        let start_ratio = band_index as f32 / num_bands as f32;
        let end_ratio = (band_index + 1) as f32 / num_bands as f32;
        let band_low = low * log_ratio.powf(start_ratio);
        let band_high = low * log_ratio.powf(end_ratio);

        let mut magnitude_sum = 0.0_f32;
        let mut bin_count = 0usize;
        for (frequency, magnitude) in spectrum {
            if *frequency >= band_low && *frequency < band_high {
                magnitude_sum += *magnitude;
                bin_count += 1;
            }
        }

        if bin_count == 0 {
            if let Some((_, magnitude)) = spectrum.iter().min_by(|a, b| {
                (a.0 - band_low)
                    .abs()
                    .partial_cmp(&(b.0 - band_low).abs())
                    .unwrap_or(Ordering::Equal)
            }) {
                magnitude_sum = *magnitude;
                bin_count = 1;
            }
        }

        let average = if bin_count == 0 {
            0.0
        } else {
            magnitude_sum / bin_count as f32
        };

        let bandwidth = bin_count.max(1) as f32;
        let eq =
            CAVA_EQ_SCALE * band_high.powf(tuning.eq_power) / bandwidth / (FFT_SIZE as f32).log2();
        *band = average * eq;
    }

    bars
}

#[cfg(target_os = "linux")]
fn configure_linux_window_backend() {
    if env::var_os("DISPLAY").is_some() && env::var_os("WAYLAND_DISPLAY").is_some() {
        env::remove_var("WAYLAND_DISPLAY");
        env::remove_var("WAYLAND_SOCKET");
        env::set_var("WINIT_UNIX_BACKEND", "x11");
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_window_backend() {}

fn app_icon() -> egui::IconData {
    let width = 64;
    let height = 64;
    let mut rgba = vec![0_u8; width * height * 4];

    let set_pixel = |rgba: &mut [u8], x: usize, y: usize, color: [u8; 4]| {
        if x >= width || y >= height {
            return;
        }
        let index = (y * width + x) * 4;
        rgba[index..index + 4].copy_from_slice(&color);
    };

    let draw_bar = |rgba: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize, bright: bool| {
        for y in y0..y1 {
            for x in x0..x1 {
                let edge = x == x0 || x + 1 == x1 || y == y0 || y + 1 == y1;
                let color = if bright {
                    if edge {
                        [242, 231, 255, 255]
                    } else {
                        [165, 110, 255, 255]
                    }
                } else if edge {
                    [223, 205, 255, 255]
                } else {
                    [126, 79, 232, 255]
                };
                set_pixel(rgba, x, y, color);
            }
        }
    };

    draw_bar(&mut rgba, 7, 38, 15, 53, false);
    draw_bar(&mut rgba, 17, 31, 24, 58, false);
    draw_bar(&mut rgba, 27, 34, 34, 55, false);
    draw_bar(&mut rgba, 38, 23, 46, 50, true);
    draw_bar(&mut rgba, 48, 31, 56, 54, false);

    draw_bar(&mut rgba, 48, 13, 56, 30, false);
    draw_bar(&mut rgba, 38, 8, 46, 28, true);
    draw_bar(&mut rgba, 28, 15, 35, 36, false);
    draw_bar(&mut rgba, 18, 23, 25, 40, false);

    egui::IconData {
        rgba,
        width: width as u32,
        height: height as u32,
    }
}

fn main() -> Result<(), eframe::Error> {
    configure_linux_window_backend();

    let options = NativeOptions {
        renderer: Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(Vec2::new(1280.0, 820.0))
            .with_min_inner_size(Vec2::new(900.0, 620.0))
            .with_icon(app_icon())
            .with_title(format!("{APP_TITLE} — default sink monitor")),
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(|_cc| Ok(Box::new(SpectrumApp::new()))),
    )
}
