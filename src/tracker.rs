//! FFB signal tracker and statistics aggregator.

use crate::parser::{FfbEvent, CLIP_THRESHOLD};
use std::collections::{HashMap, VecDeque};

pub const DEFAULT_HISTORY_CAPACITY: usize = 2500;
pub const ROLLING_WINDOW_US: u64 = 5_000_000; // 5 seconds in microseconds

#[derive(Debug, Clone)]
pub struct ForceSample {
    pub time_s: f64,
    pub level_pct: f32,
    #[allow(dead_code)]
    pub is_clipped: bool,
}

#[derive(Debug)]
pub struct FfbTracker {
    pub device_name: String,
    pub current_level: i16,
    pub current_level_pct: f32,
    pub is_currently_clipped: bool,
    pub is_clip_latched: bool,
    pub clip_latched_until_us: u64,
    pub peak_level_pct: f32,
    pub peak_timestamp_us: u64,
    pub histogram_bins: [u64; 21],

    // Timing & rate
    pub first_timestamp_us: Option<u64>,
    pub last_timestamp_us: u64,
    pub effective_hz: f32,
    pub recent_intervals_us: VecDeque<u64>,

    // Session stats
    pub constant_count: u64,
    pub clip_count: u64,
    pub min_level: i16,
    pub max_level: i16,

    // Rolling 5-second stats: (timestamp_us, is_clipped)
    pub rolling_samples: VecDeque<(u64, bool)>,
    pub rolling_clip_count: usize,

    // Effect types & commands
    pub effect_counts: HashMap<String, u64>,
    pub play_count: u64,
    pub stop_count: u64,
    pub remove_count: u64,
    pub current_gain: u32,

    // History for waveform graph
    pub history: VecDeque<ForceSample>,
    pub history_capacity: usize,
}

impl Default for FfbTracker {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_CAPACITY)
    }
}

impl FfbTracker {
    pub fn new(history_capacity: usize) -> Self {
        Self {
            device_name: "Waiting for FFB device...".to_string(),
            current_level: 0,
            current_level_pct: 0.0,
            is_currently_clipped: false,
            is_clip_latched: false,
            clip_latched_until_us: 0,
            peak_level_pct: 0.0,
            peak_timestamp_us: 0,
            histogram_bins: [0; 21],
            first_timestamp_us: None,
            last_timestamp_us: 0,
            effective_hz: 0.0,
            recent_intervals_us: VecDeque::with_capacity(64),
            constant_count: 0,
            clip_count: 0,
            min_level: 0,
            max_level: 0,
            rolling_samples: VecDeque::with_capacity(2000),
            rolling_clip_count: 0,
            effect_counts: HashMap::new(),
            play_count: 0,
            stop_count: 0,
            remove_count: 0,
            current_gain: 65535,
            history: VecDeque::with_capacity(history_capacity),
            history_capacity,
        }
    }

    pub fn reset_session(&mut self) {
        self.constant_count = 0;
        self.clip_count = 0;
        self.min_level = 0;
        self.max_level = 0;
        self.peak_level_pct = 0.0;
        self.is_clip_latched = false;
        self.clip_latched_until_us = 0;
        self.histogram_bins = [0; 21];
        self.first_timestamp_us = None;
        self.recent_intervals_us.clear();
        self.rolling_samples.clear();
        self.rolling_clip_count = 0;
        self.effect_counts.clear();
        self.play_count = 0;
        self.stop_count = 0;
        self.remove_count = 0;
        self.history.clear();
    }

    pub fn process_event(&mut self, event: FfbEvent) {
        match event {
            FfbEvent::DeviceName(name) => {
                self.device_name = name;
            }
            FfbEvent::ConstantUpload {
                timestamp_us,
                id: _,
                level,
            } => {
                if self.first_timestamp_us.is_none() {
                    self.first_timestamp_us = Some(timestamp_us);
                }

                // Interval calculation
                if self.last_timestamp_us > 0 && timestamp_us > self.last_timestamp_us {
                    let dt = timestamp_us - self.last_timestamp_us;
                    if dt < 1_000_000 {
                        // ignore huge pauses (e.g. menus)
                        if self.recent_intervals_us.len() >= 64 {
                            self.recent_intervals_us.pop_front();
                        }
                        self.recent_intervals_us.push_back(dt);

                        if !self.recent_intervals_us.is_empty() {
                            let mut sorted: Vec<u64> =
                                self.recent_intervals_us.iter().copied().collect();
                            sorted.sort_unstable();
                            let median_us = sorted[sorted.len() / 2];
                            if median_us > 0 {
                                self.effective_hz = 1_000_000.0 / (median_us as f32);
                            }
                        }
                    }
                }
                self.last_timestamp_us = timestamp_us;

                self.current_level = level;
                let pct = (level as f32 / CLIP_THRESHOLD as f32) * 100.0;
                self.current_level_pct = pct.clamp(-100.0, 100.0);

                let is_clipped = level <= -CLIP_THRESHOLD || level == CLIP_THRESHOLD;
                self.is_currently_clipped = is_clipped;

                if is_clipped {
                    self.clip_latched_until_us = timestamp_us + 400_000; // 400ms latch for human visibility
                }
                self.is_clip_latched = is_clipped || timestamp_us < self.clip_latched_until_us;

                // Histogram binning: 21 bins for -100% to +100%
                let bin = (((self.current_level_pct + 100.0) / 200.0) * 20.0)
                    .clamp(0.0, 20.0)
                    .round() as usize;
                self.histogram_bins[bin] += 1;

                // Peak hold (with 1.5s decay)
                let abs_pct = self.current_level_pct.abs();
                if abs_pct >= self.peak_level_pct
                    || timestamp_us.saturating_sub(self.peak_timestamp_us) > 1_500_000
                {
                    self.peak_level_pct = abs_pct;
                    self.peak_timestamp_us = timestamp_us;
                }

                // Min / Max
                if self.constant_count == 0 {
                    self.min_level = level;
                    self.max_level = level;
                } else {
                    self.min_level = self.min_level.min(level);
                    self.max_level = self.max_level.max(level);
                }

                self.constant_count += 1;
                if is_clipped {
                    self.clip_count += 1;
                }

                *self
                    .effect_counts
                    .entry("CONSTANT".to_string())
                    .or_insert(0) += 1;

                // Rolling 5-second window
                self.rolling_samples.push_back((timestamp_us, is_clipped));
                if is_clipped {
                    self.rolling_clip_count += 1;
                }
                while let Some(&(old_ts, old_clip)) = self.rolling_samples.front() {
                    if timestamp_us.saturating_sub(old_ts) > ROLLING_WINDOW_US {
                        self.rolling_samples.pop_front();
                        if old_clip {
                            self.rolling_clip_count = self.rolling_clip_count.saturating_sub(1);
                        }
                    } else {
                        break;
                    }
                }

                // History for waveform
                let base_ts = self.first_timestamp_us.unwrap_or(timestamp_us);
                let time_s = (timestamp_us.saturating_sub(base_ts)) as f64 / 1_000_000.0;
                if self.history.len() >= self.history_capacity {
                    self.history.pop_front();
                }
                self.history.push_back(ForceSample {
                    time_s,
                    level_pct: self.current_level_pct,
                    is_clipped,
                });
            }
            FfbEvent::OtherUpload { effect_type, .. } => {
                *self.effect_counts.entry(effect_type).or_insert(0) += 1;
            }
            FfbEvent::Gain { gain, .. } => {
                self.current_gain = gain;
            }
            FfbEvent::Play { .. } => {
                self.play_count += 1;
            }
            FfbEvent::Stop { .. } => {
                self.stop_count += 1;
            }
            FfbEvent::Remove { .. } => {
                self.remove_count += 1;
            }
            FfbEvent::Ignored => {}
        }
    }

    pub fn total_clip_percentage(&self) -> f32 {
        if self.constant_count == 0 {
            0.0
        } else {
            (self.clip_count as f32 / self.constant_count as f32) * 100.0
        }
    }

    pub fn rolling_clip_percentage(&self) -> f32 {
        let total = self.rolling_samples.len();
        if total == 0 {
            0.0
        } else {
            (self.rolling_clip_count as f32 / total as f32) * 100.0
        }
    }

    /// Single line formatted for MangoHud or status monitor:
    /// e.g.: FFB 45% [CLIP 0.2%] 229Hz
    pub fn format_status_line(&self) -> String {
        let force_abs = self.current_level_pct.abs().round() as i32;
        let clip_tag = if self.is_currently_clipped {
            " [CLIP!]".to_string()
        } else if self.rolling_clip_count > 0 {
            format!(" [clip {:.1}%]", self.rolling_clip_percentage())
        } else {
            String::new()
        };

        let hz = self.effective_hz.round() as i32;
        format!(
            "FFB {:>3}%{}{}",
            force_abs,
            clip_tag,
            if hz > 0 {
                format!(" {}Hz", hz)
            } else {
                String::new()
            }
        )
    }

    /// Calculate actionable FFB gain recommendation based on peak headroom and clipping rate
    pub fn tuning_recommendation(&self) -> (&'static str, String, String) {
        if self.constant_count < 100 {
            return (
                "ANALYZING",
                "CALC...".to_string(),
                "gathering data".to_string(),
            );
        }

        let clip_pct = self.total_clip_percentage();
        if clip_pct > 1.5 {
            let reduction = (clip_pct * 3.5).clamp(3.0, 30.0).round() as i32;
            (
                "CLIPPING HIGH",
                format!("GAIN -{}%", reduction),
                format!("clip {:.1}%", clip_pct),
            )
        } else if clip_pct > 0.1 && self.peak_level_pct >= 98.0 {
            (
                "OPTIMAL",
                "OPTIMAL".to_string(),
                format!("peak {:.0}%", self.peak_level_pct),
            )
        } else if self.peak_level_pct < 85.0 {
            let headroom = 100.0 - self.peak_level_pct;
            let increase = (headroom * 0.7).clamp(3.0, 25.0).round() as i32;
            (
                "HEADROOM LOW",
                format!("GAIN +{}%", increase),
                format!("headroom {:.0}%", headroom),
            )
        } else {
            (
                "BALANCED",
                "BALANCED".to_string(),
                format!("peak {:.0}%", self.peak_level_pct),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_clipping_stats() {
        let mut tracker = FfbTracker::new(100);

        // Upload 4 normal samples, 1 clipped sample
        for i in 1..=4 {
            tracker.process_event(FfbEvent::ConstantUpload {
                timestamp_us: i * 4000,
                id: 1,
                level: 10000,
            });
        }
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 5 * 4000,
            id: 1,
            level: 32767,
        });

        assert_eq!(tracker.constant_count, 5);
        assert_eq!(tracker.clip_count, 1);
        assert_eq!(tracker.total_clip_percentage(), 20.0);
        assert!(tracker.is_currently_clipped);
    }

    #[test]
    fn test_status_line_format() {
        let mut tracker = FfbTracker::new(100);
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 4000,
            id: 1,
            level: 16383,
        });
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 8000,
            id: 1,
            level: 16383,
        });

        let line = tracker.format_status_line();
        assert!(line.starts_with("FFB  50%"));
    }

    #[test]
    fn test_clipping_latch_behavior() {
        let mut tracker = FfbTracker::new(100);

        // 1. Normal event at 0us
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 0,
            id: 1,
            level: 10000,
        });
        assert!(!tracker.is_clip_latched);

        // 2. Clipped event at 10,000us -> should latch until 10,000 + 400,000 = 410,000us
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 10_000,
            id: 1,
            level: 32767,
        });
        assert!(tracker.is_clip_latched);
        assert_eq!(tracker.clip_latched_until_us, 410_000);

        // 3. Normal event at 200,000us (< 410,000us) -> latch remains active
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 200_000,
            id: 1,
            level: 5000,
        });
        assert!(tracker.is_clip_latched);

        // 4. Normal event at 410,001us (> 410,000us) -> latch clears
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 410_001,
            id: 1,
            level: 5000,
        });
        assert!(!tracker.is_clip_latched);
    }

    #[test]
    fn test_histogram_binning() {
        let mut tracker = FfbTracker::new(100);

        // Center zero (0) should land in bin 10
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 1000,
            id: 1,
            level: 0,
        });
        assert_eq!(tracker.histogram_bins[10], 1);

        // Full negative (-32768) should land in bin 0
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 2000,
            id: 1,
            level: -32768,
        });
        assert_eq!(tracker.histogram_bins[0], 1);

        // Full positive (+32767) should land in bin 20
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 3000,
            id: 1,
            level: 32767,
        });
        assert_eq!(tracker.histogram_bins[20], 1);

        // Half positive (~16384) should land in bin 15
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 4000,
            id: 1,
            level: 16384,
        });
        assert_eq!(tracker.histogram_bins[15], 1);
    }

    #[test]
    fn test_reset_session() {
        let mut tracker = FfbTracker::new(100);
        tracker.process_event(FfbEvent::DeviceName("MOZA R9".to_string()));
        tracker.process_event(FfbEvent::ConstantUpload {
            timestamp_us: 1000,
            id: 1,
            level: 32767,
        });

        assert_eq!(tracker.constant_count, 1);
        assert_eq!(tracker.clip_count, 1);
        assert_eq!(tracker.device_name, "MOZA R9");

        tracker.reset_session();

        assert_eq!(tracker.constant_count, 0);
        assert_eq!(tracker.clip_count, 0);
        assert_eq!(tracker.histogram_bins, [0; 21]);
        assert_eq!(tracker.peak_level_pct, 0.0);
        assert!(!tracker.is_clip_latched);
        // Device name must be preserved across session resets
        assert_eq!(tracker.device_name, "MOZA R9");
    }

    #[test]
    fn test_effective_hz() {
        let mut tracker = FfbTracker::new(100);

        // Simulate 10 updates exactly 4000us apart (250 Hz)
        for i in 1..=10 {
            tracker.process_event(FfbEvent::ConstantUpload {
                timestamp_us: i * 4000,
                id: 1,
                level: 5000,
            });
        }

        assert_eq!(tracker.effective_hz.round() as i32, 250);
    }

    #[test]
    fn test_tuning_recommendation() {
        let mut tracker = FfbTracker::new(100);

        // Few samples -> analyzing
        let (tag, primary, _) = tracker.tuning_recommendation();
        assert_eq!(tag, "ANALYZING");
        assert_eq!(primary, "CALC...");

        // High clipping (> 1.5%)
        for i in 1..=100 {
            tracker.process_event(FfbEvent::ConstantUpload {
                timestamp_us: i * 4000,
                id: 1,
                level: if i <= 10 { 32767 } else { 15000 },
            });
        }
        let (tag, primary, sub) = tracker.tuning_recommendation();
        assert_eq!(tag, "CLIPPING HIGH");
        assert!(primary.contains("GAIN -"));
        assert!(sub.contains("clip"));
    }
}
