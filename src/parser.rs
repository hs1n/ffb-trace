//! Parser for ffbwrap log streams.
//!
//! Extracts timestamp, event type, force level, gain, and device name from log lines.

pub const CLIP_THRESHOLD: i16 = 32767;

#[derive(Debug, Clone, PartialEq)]
pub enum FfbEvent {
    DeviceName(String),
    ConstantUpload {
        timestamp_us: u64,
        id: i32,
        level: i16,
    },
    OtherUpload {
        timestamp_us: u64,
        effect_type: String,
    },
    Gain {
        timestamp_us: u64,
        gain: u32,
    },
    Play {
        timestamp_us: u64,
    },
    Stop {
        timestamp_us: u64,
    },
    Remove {
        timestamp_us: u64,
    },
    Ignored,
}

pub fn parse_line(line: &str) -> Option<FfbEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Header with device name (can have timestamp or not)
    if let Some(idx) = trimmed.find("# DEVICE_NAME=") {
        let rest = &trimmed[idx + "# DEVICE_NAME=".len()..];
        let name = rest.split(',').next().unwrap_or("").trim().to_string();
        if !name.is_empty() {
            return Some(FfbEvent::DeviceName(name));
        }
    }

    // Standard log line: <timestamp_us> <message>
    let mut parts = trimmed.splitn(2, |c: char| c.is_ascii_whitespace());
    let ts_str = parts.next()?;
    let rest = parts.next()?.trim();

    let timestamp_us = ts_str.parse::<u64>().ok()?;

    if rest.starts_with("> UPLOAD") {
        // Extract type
        let etype = extract_field(rest, "type:").unwrap_or_else(|| "UNKNOWN".to_string());
        if etype == "CONSTANT" {
            let id = extract_field(rest, "id:")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(0);
            let level = extract_field(rest, "level:")
                .and_then(|v| v.parse::<i16>().ok())
                .unwrap_or(0);
            return Some(FfbEvent::ConstantUpload {
                timestamp_us,
                id,
                level,
            });
        } else {
            return Some(FfbEvent::OtherUpload {
                timestamp_us,
                effect_type: etype,
            });
        }
    }

    if rest.starts_with("> GAIN") {
        let mut gain_parts = rest.split_whitespace();
        gain_parts.next(); // ">"
        gain_parts.next(); // "GAIN"
        if let Some(val_str) = gain_parts.next() {
            if let Ok(gain) = val_str.parse::<u32>() {
                return Some(FfbEvent::Gain { timestamp_us, gain });
            }
        }
    }

    if rest.starts_with("> PLAY") {
        return Some(FfbEvent::Play { timestamp_us });
    }

    if rest.starts_with("> STOP") {
        return Some(FfbEvent::Stop { timestamp_us });
    }

    if rest.starts_with("> REMOVE") {
        return Some(FfbEvent::Remove { timestamp_us });
    }

    Some(FfbEvent::Ignored)
}

fn extract_field(haystack: &str, prefix: &str) -> Option<String> {
    let start = haystack.find(prefix)? + prefix.len();
    let rest = &haystack[start..];
    let token = rest
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| c == ',' || c == ';' || c == ')');
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_name_header() {
        let line = "000000000000 # DEVICE_NAME=Gudsen MOZA R9 Base, UPDATE_FIX=0";
        assert_eq!(
            parse_line(line),
            Some(FfbEvent::DeviceName("Gudsen MOZA R9 Base".to_string()))
        );
    }

    #[test]
    fn test_constant_upload() {
        let line = "000089318911 > UPLOAD id:1 dir:16384 length:0 delay:0 type:CONSTANT level:32767 attack_length:0";
        assert_eq!(
            parse_line(line),
            Some(FfbEvent::ConstantUpload {
                timestamp_us: 89318911,
                id: 1,
                level: 32767,
            })
        );
    }

    #[test]
    fn test_negative_constant_clipping() {
        let line = "000089318911 > UPLOAD id:1 dir:16384 type:CONSTANT level:-32767";
        assert_eq!(
            parse_line(line),
            Some(FfbEvent::ConstantUpload {
                timestamp_us: 89318911,
                id: 1,
                level: -32767,
            })
        );
    }

    #[test]
    fn test_periodic_upload() {
        let line = "000099318911 > UPLOAD id:2 dir:0 type:PERIODIC";
        assert_eq!(
            parse_line(line),
            Some(FfbEvent::OtherUpload {
                timestamp_us: 99318911,
                effect_type: "PERIODIC".to_string(),
            })
        );
    }

    #[test]
    fn test_gain() {
        let line = "000000252176 > GAIN 65535";
        assert_eq!(
            parse_line(line),
            Some(FfbEvent::Gain {
                timestamp_us: 252176,
                gain: 65535,
            })
        );
    }

    #[test]
    fn test_play_stop_remove() {
        let play_line = "000000252200 > PLAY id:1 val:1";
        assert_eq!(
            parse_line(play_line),
            Some(FfbEvent::Play {
                timestamp_us: 252200,
            })
        );

        let stop_line = "000000253000 > STOP id:1 val:0";
        assert_eq!(
            parse_line(stop_line),
            Some(FfbEvent::Stop {
                timestamp_us: 253000,
            })
        );

        let remove_line = "000000254000 > REMOVE id:1";
        assert_eq!(
            parse_line(remove_line),
            Some(FfbEvent::Remove {
                timestamp_us: 254000,
            })
        );
    }

    #[test]
    fn test_unknown_and_corrupt_lines() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("Random debug line from application"), None);
        assert_eq!(
            parse_line("000000252200 > UNKNOWN command"),
            Some(FfbEvent::Ignored)
        );
        assert_eq!(parse_line("invalid_timestamp > GAIN 100"), None);
    }
}
