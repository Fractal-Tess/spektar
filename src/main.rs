use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, Stream, StreamConfig,
};
use eframe::{egui, NativeOptions};
use egui::{Color32, Ui};
use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const HISTORY_SIZE: usize = 50;
const NUM_BANDS: usize = 40;

struct SpectrumApp {
    spectrum_data: Arc<Mutex<VecDeque<Vec<f32>>>>,
    audio_stream: Option<Stream>,
    sample_buffer: Arc<Mutex<Vec<f32>>>,
}

impl Default for SpectrumApp {
    fn default() -> Self {
        Self {
            spectrum_data: Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_SIZE))),
            audio_stream: None,
            sample_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl eframe::App for SpectrumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process audio data if available
        if let Ok(mut buffer) = self.sample_buffer.try_lock() {
            if buffer.len() >= 1024 {
                // Take samples for FFT
                let samples: Vec<f32> = buffer.drain(0..1024).collect();
                
                // Convert to complex numbers for FFT
                let hann_window = spectrum_analyzer::windows::hann_window(&samples);
                let spectrum_result = samples_fft_to_spectrum(
                    &hann_window,
                    44100,
                    FrequencyLimit::Range(20.0, 20000.0),
                    None,
                );

                if let Ok(spectrum) = spectrum_result {
                    // Convert spectrum to bands - first convert OrderableF32 to f32
                    let spectrum_data: Vec<(f32, f32)> = spectrum
                        .data()
                        .iter()
                        .map(|(freq, val)| (freq.val(), val.val()))
                        .collect();
                    let bands = convert_spectrum_to_bands(&spectrum_data, NUM_BANDS);
                    
                    if let Ok(mut spectrum_data) = self.spectrum_data.lock() {
                        spectrum_data.push_back(bands);
                        if spectrum_data.len() > HISTORY_SIZE {
                            spectrum_data.pop_front();
                        }
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Spektar - Audio Spectrum Visualizer");

            // Draw the spectrum visualization
            if let Ok(spectrum_data) = self.spectrum_data.lock() {
                self.draw_spectrum(ui, &spectrum_data);
            }
        });

        // Request continuous repainting
        ctx.request_repaint();
    }
}

impl SpectrumApp {
    fn init_audio(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        
        // Get the default input device
        let device = host
            .default_input_device()
            .ok_or("No input device available")?;

        println!("Using input device: {}", device.name()?);

        // Get the default input config
        let config = device.default_input_config()?;
        println!("Default input config: {:?}", config);

        let sample_format = config.sample_format();
        let config: StreamConfig = config.into();
        
        let sample_buffer = Arc::clone(&self.sample_buffer);
        
        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buffer) = sample_buffer.lock() {
                        buffer.extend_from_slice(data);
                        // Keep buffer size reasonable
                        if buffer.len() > 4096 {
                            let excess = buffer.len() - 4096;
                            buffer.drain(0..excess);
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buffer) = sample_buffer.lock() {
                        let float_data: Vec<f32> = data.iter().map(|&x| x as f32 / i16::MAX as f32).collect();
                        buffer.extend_from_slice(&float_data);
                        // Keep buffer size reasonable
                        if buffer.len() > 4096 {
                            let excess = buffer.len() - 4096;
                            buffer.drain(0..excess);
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::U16 => device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buffer) = sample_buffer.lock() {
                        let float_data: Vec<f32> = data.iter().map(|&x| (x as f32 / u16::MAX as f32) * 2.0 - 1.0).collect();
                        buffer.extend_from_slice(&float_data);
                        // Keep buffer size reasonable
                        if buffer.len() > 4096 {
                            let excess = buffer.len() - 4096;
                            buffer.drain(0..excess);
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            _ => return Err("Unsupported sample format".into()),
        };

        stream.play()?;
        self.audio_stream = Some(stream);

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

fn convert_spectrum_to_bands(spectrum: &[(f32, f32)], num_bands: usize) -> Vec<f32> {
    let mut bands = vec![0.0; num_bands];
    let spectrum_len = spectrum.len();

    if spectrum_len == 0 {
        return bands;
    }

    // Map the spectrum to our bands using a logarithmic scale
    for (i, band) in bands.iter_mut().enumerate() {
        let start_idx = ((i as f32 / num_bands as f32).powf(2.0) * spectrum_len as f32) as usize;
        let end_idx = (((i + 1) as f32 / num_bands as f32).powf(2.0) * spectrum_len as f32) as usize;
        let end_idx = end_idx.min(spectrum_len);

        if start_idx < end_idx {
            let sum: f32 = spectrum[start_idx..end_idx]
                .iter()
                .map(|f| f.1)
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
