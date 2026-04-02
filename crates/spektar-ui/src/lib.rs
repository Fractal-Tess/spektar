use eframe::{
    egui::{self, Color32, Pos2, Rect, Stroke, Vec2},
    NativeOptions, Renderer,
};
use spektar_audio::{
    latest_sample_window, new_audio_status, new_capture_stop, new_sample_buffer, new_sample_rate,
    spawn_audio_capture, AudioStatus, CaptureStopFlag, DiagnosticConfig, SharedAudioStatus,
    SharedSampleBuffer, SharedSampleRate,
};
use spektar_spectrum::{AudioProcessStats, ResponsePreset, SpectrumProcessor, FFT_SIZE, NUM_BANDS};
use std::{
    env,
    sync::{atomic::Ordering as AtomicOrdering, Arc},
    thread,
    time::{Duration, Instant},
};

const APP_TITLE: &str = "Spektar";
const DEFAULT_SAMPLE_RATE: u32 = 44_100;

pub fn run_app() -> Result<(), eframe::Error> {
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

struct SpectrumApp {
    sample_buffer: SharedSampleBuffer,
    audio_status: SharedAudioStatus,
    sample_rate: SharedSampleRate,
    processor: SpectrumProcessor,
    target_fps: u32,
    analysis_hz: u32,
    diagnostic_mode: bool,
    started_at: Instant,
    last_frame_at: Instant,
    last_analysis_at: Instant,
    capture_stop: CaptureStopFlag,
    capture_thread: Option<thread::JoinHandle<()>>,
}

impl SpectrumApp {
    fn new() -> Self {
        let audio_status = new_audio_status();
        let sample_buffer = new_sample_buffer();
        let sample_rate = new_sample_rate(DEFAULT_SAMPLE_RATE);
        let capture_stop = new_capture_stop();
        let debug_logging = env::var_os("SPEKTAR_DEBUG").is_some();
        let diagnostic_mode = env::var_os("SPEKTAR_DIAGNOSTIC").is_some();
        let diagnostic = Arc::new(DiagnosticConfig::new(diagnostic_mode));
        let started_at = diagnostic.started_at();
        let capture_thread = spawn_audio_capture(
            Arc::clone(&sample_buffer),
            Arc::clone(&sample_rate),
            Arc::clone(&audio_status),
            Arc::clone(&capture_stop),
            DEFAULT_SAMPLE_RATE,
            Arc::clone(&diagnostic),
        );

        Self {
            sample_buffer,
            audio_status,
            sample_rate,
            processor: SpectrumProcessor::new(debug_logging),
            target_fps: 60,
            analysis_hz: 60,
            diagnostic_mode,
            started_at,
            last_frame_at: started_at,
            last_analysis_at: started_at,
            capture_stop,
            capture_thread: Some(capture_thread),
        }
    }

    fn process_audio(&mut self) -> AudioProcessStats {
        let sample_rate = self
            .sample_rate
            .lock()
            .map(|rate| *rate)
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        let window = latest_sample_window(&self.sample_buffer, FFT_SIZE);

        if let Some(samples) = window.samples {
            return self.processor.process_samples(
                &samples,
                sample_rate,
                window.buffer_len_before,
                window.drained_samples,
                window.lock_wait,
            );
        }

        AudioProcessStats::skipped(
            window.buffer_len_before,
            window.lock_wait,
            window.drained_samples,
        )
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

        if self
            .processor
            .current_bars()
            .iter()
            .all(|value| *value <= 0.0001)
        {
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
                self.processor.preset().label().to_uppercase()
            ),
            egui::FontId::monospace(16.0),
            Color32::from_rgb(220, 228, 238),
        );
    }

    fn draw_bars(&self, painter: &egui::Painter, rect: Rect) {
        let band_width = rect.width() / NUM_BANDS as f32;

        for (index, value) in self.processor.current_bars().iter().enumerate() {
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
        let mut selected_preset = self.processor.preset();
        let mut lerp_smoothing = self.processor.lerp_smoothing();

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Response").strong());
            for preset in ResponsePreset::ALL {
                let selected = selected_preset == preset;
                if ui.selectable_label(selected, preset.label()).clicked() {
                    selected_preset = preset;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(&mut lerp_smoothing, 0.0..=0.85).text("Lerp"));
            ui.add(egui::Slider::new(&mut self.target_fps, 15..=144).text("FPS"));
            ui.add(egui::Slider::new(&mut self.analysis_hz, 10..=120).text("Sampling"));
        });
        ui.small(selected_preset.description());

        self.processor.set_preset(selected_preset);
        self.processor.set_lerp_smoothing(lerp_smoothing);
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
            AudioProcessStats::skipped(0, Duration::ZERO, 0)
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
                self.processor.sensitivity(),
                self.processor.current_bars().iter().copied().fold(0.0_f32, f32::max)
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
                                self.draw_status(ui, &status);
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
                self.processor
                    .current_bars()
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max),
                self.processor
                    .current_bars()
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

impl SpectrumApp {
    fn draw_status(&self, ui: &mut egui::Ui, status: &AudioStatus) {
        ui.label(format!("{} • {}", status.backend, status.source));
        if let Some(error) = &status.error {
            ui.colored_label(Color32::from_rgb(255, 120, 120), error);
        } else {
            ui.label(&status.message);
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
