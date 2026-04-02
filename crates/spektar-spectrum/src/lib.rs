use spectrum_analyzer::{samples_fft_to_spectrum, FrequencyLimit};
use std::{cmp::Ordering, time::Duration};

pub const NUM_BANDS: usize = 40;
pub const FFT_SIZE: usize = 2048;
const CAVA_EQ_SCALE: f32 = 1.0 / 4096.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResponsePreset {
    Balanced,
    Monstercat,
    Punchy,
    Smooth,
}

impl ResponsePreset {
    pub const ALL: [Self; 4] = [Self::Balanced, Self::Monstercat, Self::Punchy, Self::Smooth];

    pub fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Monstercat => "Monstercat",
            Self::Punchy => "Punchy",
            Self::Smooth => "Smooth",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Balanced => "general-purpose Cava-like response",
            Self::Monstercat => "stronger low-end and smoother desktop-style decay",
            Self::Punchy => "faster hits with less lingering memory",
            Self::Smooth => "slower, softer response with gentle decay",
        }
    }

    pub fn tuning(self) -> ResponseTuning {
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
pub struct ResponseTuning {
    pub low_cutoff_hz: f32,
    pub high_cutoff_hz: f32,
    pub eq_power: f32,
    pub attack: f32,
    pub release: f32,
    pub bar_ceiling: f32,
    pub sensitivity_rise: f32,
    pub sensitivity_fall: f32,
    pub initial_sensitivity: f32,
}

pub struct AudioProcessStats {
    pub buffer_len_before: usize,
    pub lock_wait: Duration,
    pub drained_samples: usize,
    pub fft_duration: Duration,
    pub raw_max: f32,
    pub had_fft: bool,
}

impl AudioProcessStats {
    pub fn skipped(buffer_len_before: usize, lock_wait: Duration, drained_samples: usize) -> Self {
        Self {
            buffer_len_before,
            lock_wait,
            drained_samples,
            fft_duration: Duration::ZERO,
            raw_max: 0.0,
            had_fft: false,
        }
    }
}

pub struct SpectrumProcessor {
    preset: ResponsePreset,
    current_bars: Vec<f32>,
    sensitivity: f32,
    lerp_smoothing: f32,
    debug_logging: bool,
    debug_frame: u64,
}

impl SpectrumProcessor {
    pub fn new(debug_logging: bool) -> Self {
        let preset = ResponsePreset::Balanced;
        let tuning = preset.tuning();

        Self {
            preset,
            current_bars: vec![0.0; NUM_BANDS],
            sensitivity: tuning.initial_sensitivity,
            lerp_smoothing: 0.12,
            debug_logging,
            debug_frame: 0,
        }
    }

    pub fn preset(&self) -> ResponsePreset {
        self.preset
    }

    pub fn set_preset(&mut self, preset: ResponsePreset) {
        if self.preset != preset {
            self.preset = preset;
            self.reset_response_state();
        }
    }

    pub fn current_bars(&self) -> &[f32] {
        &self.current_bars
    }

    pub fn sensitivity(&self) -> f32 {
        self.sensitivity
    }

    pub fn lerp_smoothing(&self) -> f32 {
        self.lerp_smoothing
    }

    pub fn set_lerp_smoothing(&mut self, lerp_smoothing: f32) {
        self.lerp_smoothing = lerp_smoothing;
    }

    pub fn process_samples(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        buffer_len_before: usize,
        drained_samples: usize,
        lock_wait: Duration,
    ) -> AudioProcessStats {
        if samples.len() != FFT_SIZE {
            return AudioProcessStats::skipped(buffer_len_before, lock_wait, drained_samples);
        }

        let tuning = self.preset.tuning();
        let hann_window = spectrum_analyzer::windows::hann_window(samples);
        let fft_started = std::time::Instant::now();
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
                drained_samples,
                fft_duration: fft_started.elapsed(),
                raw_max,
                had_fft: true,
            };
        }

        AudioProcessStats {
            buffer_len_before,
            lock_wait,
            drained_samples,
            fft_duration: fft_started.elapsed(),
            raw_max: 0.0,
            had_fft: false,
        }
    }

    fn reset_response_state(&mut self) {
        let tuning = self.preset.tuning();
        self.current_bars.fill(0.0);
        self.sensitivity = tuning.initial_sensitivity;
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
