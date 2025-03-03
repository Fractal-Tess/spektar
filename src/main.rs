use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat,
};
use eframe::{egui, NativeOptions};
use egui::{Color32, Ui};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use audioviz::{
    io::{Device, Input},
    spectrum::{
        config::{Interpolation, ProcessorConfig, StreamConfig},
        stream::Stream,
        Frequency,
    },
};

const HISTORY_SIZE: usize = 50;
const NUM_BANDS: usize = 40;

struct SpectrumApp {
    spectrum_data: Arc<Mutex<VecDeque<Vec<f32>>>>,
    audio_stream: Option<Stream>,
    audio_receiver: Option<audioviz::io::Receiver>,
}

impl Default for SpectrumApp {
    fn default() -> Self {
        Self {
            spectrum_data: Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_SIZE))),
            audio_stream: None,
            audio_receiver: None,
        }
    }
}

impl eframe::App for SpectrumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Update audio data
        if let Some(receiver) = &self.audio_receiver {
            if let Some(new_data) = receiver.pull_data() {
                if let Some(stream) = &mut self.audio_stream {
                    stream.push_data(new_data);
                    stream.update();

                    if let Ok(mut spectrum_data) = self.spectrum_data.lock() {
                        let frequencies = stream.get_frequencies();
                        if !frequencies.is_empty() {
                            // Convert frequencies to our band format
                            let bands = convert_frequencies_to_bands(&frequencies[0], NUM_BANDS);
                            spectrum_data.push_back(bands);
                            if spectrum_data.len() > HISTORY_SIZE {
                                spectrum_data.pop_front();
                            }
                        }
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Spektar - Audio Spectrum Visualizer");

            // Draw the spectrum visualization
            let spectrum_data = self.spectrum_data.lock().unwrap();
            self.draw_spectrum(ui, &spectrum_data);
        });

        // Request continuous repainting
        ctx.request_repaint();
    }
}

impl SpectrumApp {
    fn init_audio(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut audio_input = Input::new();

        // Get the default output device
        let devices = audio_input.fetch_devices()?;
        println!("Available audio devices:");
        for (id, device) in devices.iter().enumerate() {
            println!("{id}\t{device}");
        }

        // Initialize audio input with default device
        let (channel_count, _sampling_rate, audio_receiver) =
            audio_input.init(&Device::Default, Some(1024))?;

        let stream_config = StreamConfig {
            channel_count,
            gravity: Some(2.0),
            fft_resolution: 1024 * 3,
            processor: ProcessorConfig {
                frequency_bounds: [20, 20_000],
                interpolation: Interpolation::Cubic,
                volume: 0.4,
                resolution: Some(NUM_BANDS),
                ..ProcessorConfig::default()
            },
            ..StreamConfig::default()
        };

        let stream = Stream::new(stream_config);

        self.audio_stream = Some(stream);
        self.audio_receiver = Some(audio_receiver);

        Ok(())
    }

    fn draw_spectrum(&self, ui: &mut Ui, spectrum_data: &VecDeque<Vec<f32>>) {
        if spectrum_data.is_empty() {
            ui.label("Waiting for audio data...");
            return;
        }

        let height = 200.0;
        let width = ui.available_width();
        let band_width = width / NUM_BANDS as f32;

        let (response, painter) =
            ui.allocate_painter(egui::vec2(width, height), egui::Sense::hover());

        let rect = response.rect;

        // Draw bars for each frequency band in the most recent spectrum
        if let Some(current_spectrum) = spectrum_data.back() {
            for (i, &value) in current_spectrum.iter().enumerate() {
                let normalized_value = (value.clamp(0.0, 1.0) * height).round();
                let x = rect.left() + (i as f32 * band_width);

                // Create a color gradient from green to red based on intensity
                let hue = 120.0 - (value * 120.0); // 120° is green, 0° is red
                let color = Color32::from_rgb(
                    ((1.0 - (hue / 120.0).clamp(0.0, 1.0)) * 255.0) as u8,
                    ((hue / 120.0).clamp(0.0, 1.0) * 255.0) as u8,
                    0,
                );

                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.bottom() - normalized_value),
                        egui::vec2(band_width.max(1.0) - 2.0, normalized_value),
                    ),
                    0.0,
                    color,
                );
            }
        }

        // Draw history as fading bars
        let alpha_step = 1.0 / HISTORY_SIZE as f32;
        for (history_idx, spectrum) in spectrum_data.iter().enumerate() {
            let alpha = 0.5 * (1.0 - (history_idx as f32 * alpha_step));
            if alpha <= 0.05 {
                continue;
            }

            for (i, &value) in spectrum.iter().enumerate() {
                let normalized_value = (value.clamp(0.0, 1.0) * height * 0.5).round();
                let x = rect.left() + (i as f32 * band_width);

                // Create a fading color with decreasing alpha for history
                let hue = 240.0 - (value * 120.0); // Blue to purple
                let color = Color32::from_rgba_premultiplied(
                    100,
                    100,
                    ((hue / 240.0).clamp(0.0, 1.0) * 255.0) as u8,
                    (alpha * 255.0) as u8,
                );

                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.bottom() - normalized_value),
                        egui::vec2(band_width.max(1.0) - 2.0, normalized_value),
                    ),
                    0.0,
                    color,
                );
            }
        }
    }
}

fn convert_frequencies_to_bands(frequencies: &[Frequency], num_bands: usize) -> Vec<f32> {
    let mut bands = vec![0.0; num_bands];
    let freq_len = frequencies.len();

    if freq_len == 0 {
        return bands;
    }

    // Map the frequencies to our bands using a logarithmic scale
    for (i, band) in bands.iter_mut().enumerate() {
        let start_idx = ((i as f32 / num_bands as f32).powf(2.0) * freq_len as f32) as usize;
        let end_idx = (((i + 1) as f32 / num_bands as f32).powf(2.0) * freq_len as f32) as usize;
        let end_idx = end_idx.min(freq_len);

        if start_idx < end_idx {
            let sum: f32 = frequencies[start_idx..end_idx]
                .iter()
                .map(|f| f.volume)
                .sum();
            *band = (sum / (end_idx - start_idx) as f32).clamp(0.0, 1.0);
        }
    }

    bands
}

fn main() -> Result<(), eframe::Error> {
    // Initialize app
    let mut app = SpectrumApp::default();

    // Initialize audio
    if let Err(err) = app.init_audio() {
        eprintln!("Error initializing audio: {}", err);
    }

    // Run the GUI
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("Spektar - Audio Spectrum Visualizer"),
        ..Default::default()
    };

    eframe::run_native(
        "Spektar - Audio Spectrum Visualizer",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}
