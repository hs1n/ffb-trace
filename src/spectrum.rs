//! Vibration spectrum analysis (FFT) for force feedback telemetry.
//!
//! Decomposes real-time force feedback time-series data into frequency components,
//! identifying dominant vibration peaks and energy distribution across motorsport
//! frequency bands (Steering/SAT, Chassis/Curbs, Road Texture/Scrub, Engine/ABS).

use std::collections::VecDeque;
use std::f32::consts::PI;

use crate::tracker::ForceSample;

pub const SPECTRUM_WINDOW_S: f64 = 1.0;
pub const FFT_SIZE: usize = 256;
pub const MAX_SPECTRUM_FREQ_HZ: f32 = 100.0;

/// Individual frequency bin in the spectrum.
#[derive(Debug, Clone, PartialEq)]
pub struct FrequencyBin {
    pub freq_hz: f32,
    pub magnitude_pct: f32,
}

/// Distribution of vibration energy across standard motorsport frequency bands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VibrationBands {
    /// 0 - 4 Hz: Steering resistance, self-aligning torque (SAT), vehicle load transfer.
    pub steering_pct: f32,
    /// 4 - 15 Hz: Chassis roll/pitch, bumps, curb strike rumble.
    pub chassis_curbs_pct: f32,
    /// 15 - 40 Hz: Tire scrub, slip angle vibrations, road surface grain/tarmac texture.
    pub road_texture_pct: f32,
    /// 40+ Hz: Engine harmonics, ABS pulsation, drivetrain lash/gearbox shudder.
    pub high_freq_pct: f32,
}

/// Full spectrum analysis result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpectrumAnalysis {
    /// Discrete frequency bins (e.g., 0 to 100 Hz).
    pub bins: Vec<FrequencyBin>,
    /// Energy percentage breakdown by frequency band.
    pub bands: VibrationBands,
    /// Dominant AC vibration frequency (excluding DC steering baseline).
    pub dominant_freq_hz: f32,
    /// Amplitude of the dominant vibration frequency (% FFB).
    pub dominant_magnitude_pct: f32,
    /// Effective sampling rate during the analysis window.
    pub sample_rate_hz: f32,
}

/// In-place Cooley-Tukey Radix-2 Decimation-In-Time FFT.
/// `data` contains (real, imag) pairs. Length must be a power of 2.
pub fn fft_radix2(data: &mut [(f32, f32)]) {
    let n = data.len();
    assert!(n.is_power_of_two(), "FFT size must be a power of two");

    // Bit-reversal permutation
    let mut j = 0;
    for i in 0..n {
        if i < j {
            data.swap(i, j);
        }
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
    }

    // Butterfly stages
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let angle = -2.0 * PI / (len as f32);
        let w_step = (angle.cos(), angle.sin());

        for i in (0..n).step_by(len) {
            let mut w = (1.0_f32, 0.0_f32);
            for k in 0..half {
                let u = data[i + k];
                let v = (
                    data[i + k + half].0 * w.0 - data[i + k + half].1 * w.1,
                    data[i + k + half].0 * w.1 + data[i + k + half].1 * w.0,
                );
                data[i + k] = (u.0 + v.0, u.1 + v.1);
                data[i + k + half] = (u.0 - v.0, u.1 - v.1);
                w = (
                    w.0 * w_step.0 - w.1 * w_step.1,
                    w.0 * w_step.1 + w.1 * w_step.0,
                );
            }
        }
        len <<= 1;
    }
}

/// Analyze vibration spectrum from the recent history of force samples.
pub fn analyze_spectrum(history: &VecDeque<ForceSample>) -> SpectrumAnalysis {
    if history.len() < 16 {
        return SpectrumAnalysis::default();
    }

    let t_end = history.back().map(|s| s.time_s).unwrap_or(0.0);
    let t_start = t_end - SPECTRUM_WINDOW_S;

    // Filter samples in the analysis window
    let window_samples: Vec<&ForceSample> = history
        .iter()
        .filter(|s| s.time_s >= t_start && s.time_s <= t_end)
        .collect();

    if window_samples.len() < 16 {
        return SpectrumAnalysis::default();
    }

    let span = window_samples.last().unwrap().time_s - window_samples.first().unwrap().time_s;
    if span < 0.2 {
        return SpectrumAnalysis::default();
    }

    let n = FFT_SIZE;
    let sample_rate = (n as f64 / span) as f32;

    // Resample uniformly across the window using linear interpolation
    let mut uniform_samples = vec![0.0_f32; n];
    let mut sample_idx = 0;

    for (i, sample) in uniform_samples.iter_mut().enumerate() {
        let t = window_samples.first().unwrap().time_s + (i as f64 / (n - 1) as f64) * span;

        while sample_idx + 1 < window_samples.len() && window_samples[sample_idx + 1].time_s < t {
            sample_idx += 1;
        }

        if sample_idx + 1 >= window_samples.len() {
            *sample = window_samples.last().unwrap().level_pct;
        } else {
            let s0 = window_samples[sample_idx];
            let s1 = window_samples[sample_idx + 1];
            let dt = s1.time_s - s0.time_s;
            if dt > 1e-9 {
                let frac = ((t - s0.time_s) / dt) as f32;
                *sample = s0.level_pct + frac * (s1.level_pct - s0.level_pct);
            } else {
                *sample = s0.level_pct;
            }
        }
    }

    // Separate DC steering bias from AC vibration
    let dc_mean: f32 = uniform_samples.iter().sum::<f32>() / (n as f32);

    // Apply Hann window to AC component
    let mut complex_buffer = vec![(0.0_f32, 0.0_f32); n];
    for i in 0..n {
        let ac_val = uniform_samples[i] - dc_mean;
        let hann = 0.5 * (1.0 - (2.0 * PI * i as f32 / (n - 1) as f32).cos());
        complex_buffer[i] = (ac_val * hann, 0.0);
    }

    // Execute FFT
    fft_radix2(&mut complex_buffer);

    // Calculate single-sided amplitude spectrum
    let half_n = n / 2;
    let mut bins = Vec::with_capacity(half_n);

    // DC component
    let dc_mag = dc_mean.abs();
    bins.push(FrequencyBin {
        freq_hz: 0.0,
        magnitude_pct: dc_mag,
    });

    let mut dominant_freq_hz = 0.0;
    let mut dominant_magnitude_pct = 0.0;

    // AC components (amplitude scaling: 2 / N for single-sided, and * 2 for Hann window)
    let hann_scale = 4.0 / (n as f32);
    for (k, complex) in complex_buffer.iter().enumerate().take(half_n).skip(1) {
        let freq_hz = (k as f32 * sample_rate) / (n as f32);
        if freq_hz > MAX_SPECTRUM_FREQ_HZ {
            break;
        }

        let re = complex.0;
        let im = complex.1;
        let mag = (re * re + im * im).sqrt() * hann_scale;

        if mag > dominant_magnitude_pct {
            dominant_magnitude_pct = mag;
            dominant_freq_hz = freq_hz;
        }

        bins.push(FrequencyBin {
            freq_hz,
            magnitude_pct: mag,
        });
    }

    // Band energy breakdown
    let mut e_steering = dc_mag * dc_mag;
    let mut e_chassis_curbs = 0.0_f32;
    let mut e_road_texture = 0.0_f32;
    let mut e_high = 0.0_f32;

    for bin in &bins {
        let energy = bin.magnitude_pct * bin.magnitude_pct;
        if bin.freq_hz < 4.0 {
            if bin.freq_hz > 0.0 {
                e_steering += energy;
            }
        } else if bin.freq_hz < 15.0 {
            e_chassis_curbs += energy;
        } else if bin.freq_hz < 40.0 {
            e_road_texture += energy;
        } else {
            e_high += energy;
        }
    }

    let total_energy = e_steering + e_chassis_curbs + e_road_texture + e_high;
    let bands = if total_energy > 1e-4 {
        VibrationBands {
            steering_pct: (e_steering / total_energy) * 100.0,
            chassis_curbs_pct: (e_chassis_curbs / total_energy) * 100.0,
            road_texture_pct: (e_road_texture / total_energy) * 100.0,
            high_freq_pct: (e_high / total_energy) * 100.0,
        }
    } else {
        VibrationBands::default()
    };

    SpectrumAnalysis {
        bins,
        bands,
        dominant_freq_hz,
        dominant_magnitude_pct,
        sample_rate_hz: sample_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fft_radix2_impulse() {
        let mut data = vec![(0.0, 0.0); 8];
        data[0] = (1.0, 0.0); // delta impulse
        fft_radix2(&mut data);
        for pt in data {
            assert!((pt.0 - 1.0).abs() < 1e-5);
            assert!(pt.1.abs() < 1e-5);
        }
    }

    #[test]
    fn test_fft_radix2_sine_wave() {
        let n = 128;
        let mut data = vec![(0.0, 0.0); n];
        let freq_bin = 8; // exactly 8 cycles in 128 samples
        for i in 0..n {
            let val = (2.0 * PI * freq_bin as f32 * i as f32 / n as f32).sin();
            data[i] = (val, 0.0);
        }
        fft_radix2(&mut data);

        let mag_8 = (data[freq_bin].0.powi(2) + data[freq_bin].1.powi(2)).sqrt() / (n as f32 / 2.0);
        assert!((mag_8 - 1.0).abs() < 0.05);
    }

    #[test]
    fn test_analyze_spectrum_sine_vibration() {
        let mut history = VecDeque::new();
        let sample_rate = 256.0;
        let target_freq = 12.0; // 12 Hz curb vibration
        let duration = 1.0;
        let total_samples = (sample_rate * duration) as usize;

        for i in 0..total_samples {
            let t = i as f64 / sample_rate as f64;
            // 20% DC steering bias + 30% 12Hz curb vibration
            let level = 20.0 + 30.0 * (2.0 * PI * target_freq * t as f32).sin();
            history.push_back(ForceSample {
                time_s: t,
                level_pct: level,
                is_clipped: false,
            });
        }

        let spectrum = analyze_spectrum(&history);
        assert!(!spectrum.bins.is_empty());
        assert!((spectrum.dominant_freq_hz - target_freq).abs() <= 1.5);
        assert!((spectrum.dominant_magnitude_pct - 30.0).abs() <= 3.0);
        assert!(spectrum.bands.chassis_curbs_pct > 10.0);
    }

    #[test]
    fn test_analyze_spectrum_empty() {
        let history = VecDeque::new();
        let result = analyze_spectrum(&history);
        assert!(result.bins.is_empty());
        assert_eq!(result.dominant_freq_hz, 0.0);
    }
}
