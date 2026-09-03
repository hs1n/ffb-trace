//! On-demand game runner with automatic wheel detection and preload injection.

use directories::ProjectDirs;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct DetectedDevice {
    pub name: String,
    #[allow(dead_code)]
    pub path: PathBuf,
    pub major: u32,
    pub minor: u32,
}

/// Automatically detect connected sim racing wheel in /dev/input/by-id/
pub fn detect_wheel_device(explicit_path: Option<&Path>) -> Result<DetectedDevice, String> {
    if let Some(path) = explicit_path {
        if path.exists() {
            return resolve_device(path);
        }
        return Err(format!(
            "Specified device does not exist: {}",
            path.display()
        ));
    }

    let by_id = Path::new("/dev/input/by-id");
    if !by_id.exists() {
        return Err("Directory /dev/input/by-id does not exist".to_string());
    }

    let entries =
        fs::read_dir(by_id).map_err(|e| format!("Cannot read /dev/input/by-id: {}", e))?;
    let mut candidates = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if name.ends_with("-event-joystick") {
            candidates.push((name, path));
        }
    }

    if candidates.is_empty() {
        return Err("No joystick event devices found in /dev/input/by-id/".to_string());
    }

    // Sort by known direct drive / wheel vendors
    candidates.sort_by_key(|(name, _)| vendor_priority(name));

    let (name, path) = &candidates[0];
    println!("Auto-detected wheel device: {}", name);
    resolve_device(path)
}

pub fn vendor_priority(name: &str) -> u8 {
    let n = name.to_uppercase();
    if n.contains("MOZA") || n.contains("GUDSEN") {
        0
    } else if n.contains("FANATEC")
        || n.contains("LOGITECH")
        || n.contains("THRUSTMASTER")
        || n.contains("SIMUCUBE")
        || n.contains("CAMMUS")
    {
        1
    } else {
        2
    }
}

pub fn clean_device_name(file_name: &str) -> String {
    file_name
        .replace("usb-", "")
        .replace("-event-joystick", "")
        .replace('_', " ")
}

/// Obfuscate long serial numbers (e.g. 8+ alphanumeric characters with digits)
pub fn mask_serial(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut current_word = String::new();

    let flush_word = |word: &mut String, res: &mut String| {
        if !word.is_empty() {
            let digit_count = word.chars().filter(|c| c.is_ascii_digit()).count();
            let is_serial = word.len() >= 8
                && digit_count >= 3
                && word.chars().all(|c| c.is_ascii_alphanumeric());
            if is_serial {
                res.push_str("••••••••");
            } else {
                res.push_str(word);
            }
            word.clear();
        }
    };

    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            current_word.push(c);
        } else {
            flush_word(&mut current_word, &mut result);
            result.push(c);
        }
    }
    flush_word(&mut current_word, &mut result);
    result
}

fn resolve_device(path: &Path) -> Result<DetectedDevice, String> {
    let meta = fs::metadata(path).map_err(|e| format!("Cannot stat {}: {}", path.display(), e))?;
    let rdev = meta.rdev();
    let major = ((rdev >> 8) & 0xfff) as u32;
    let minor = ((rdev & 0xff) | ((rdev >> 12) & 0xfff00)) as u32;

    let display_name = path
        .file_name()
        .map(|f| clean_device_name(&f.to_string_lossy()))
        .unwrap_or_default();

    Ok(DetectedDevice {
        name: display_name,
        path: path.to_path_buf(),
        major,
        minor,
    })
}

const EMBEDDED_LIB_X86_64: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/libffbwrapper-x86_64.so"));
const EMBEDDED_LIB_I386: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libffbwrapper-i386.so"));

/// Ensure embedded preload libraries are extracted to XDG data dir (~/.local/share/ffb-trace/lib/)
pub fn ensure_preload_libraries() -> Result<(PathBuf, Option<PathBuf>), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = if let Some(proj) = ProjectDirs::from("", "", "ffb-trace") {
        proj.data_local_dir().join("lib")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share/ffb-trace/lib")
    } else {
        PathBuf::from("/tmp/ffb-trace/lib")
    };

    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create lib directory {}: {}", dir.display(), e))?;

    let path_x86_64 = dir.join("libffbwrapper-x86_64.so");
    let needs_write_64 = !path_x86_64.exists()
        || fs::metadata(&path_x86_64)
            .map(|m| m.len() != EMBEDDED_LIB_X86_64.len() as u64)
            .unwrap_or(true);

    if needs_write_64 {
        fs::write(&path_x86_64, EMBEDDED_LIB_X86_64)
            .map_err(|e| format!("Failed to write {}: {}", path_x86_64.display(), e))?;
        let _ = fs::set_permissions(&path_x86_64, fs::Permissions::from_mode(0o755));
    }

    let has_i386 = if !EMBEDDED_LIB_I386.is_empty() {
        let path_i386 = dir.join("libffbwrapper-i386.so");
        let needs_write_32 = !path_i386.exists()
            || fs::metadata(&path_i386)
                .map(|m| m.len() != EMBEDDED_LIB_I386.len() as u64)
                .unwrap_or(true);

        if needs_write_32 {
            let _ = fs::write(&path_i386, EMBEDDED_LIB_I386);
            let _ = fs::set_permissions(&path_i386, fs::Permissions::from_mode(0o755));
        }
        Some(path_i386)
    } else {
        None
    };

    Ok((path_x86_64, has_i386))
}

/// Run a command with FFB interception preloaded
pub fn run_preloaded_command(
    command_and_args: &[String],
    device: &DetectedDevice,
    log_file: &Path,
) -> Result<std::process::Child, String> {
    if command_and_args.is_empty() {
        return Err("No command specified to run".to_string());
    }

    let (x86_64, i386_opt) = ensure_preload_libraries()?;
    let mut preload_str = x86_64.display().to_string();
    if let Some(ref i386) = i386_opt {
        preload_str = format!("{} {}", preload_str, i386.display());
    }
    if let Ok(existing_preload) = std::env::var("LD_PRELOAD") {
        if !existing_preload.trim().is_empty() {
            preload_str = format!("{} {}", preload_str, existing_preload);
        }
    }

    let mut cmd = Command::new(&command_and_args[0]);
    if command_and_args.len() > 1 {
        cmd.args(&command_and_args[1..]);
    }

    cmd.env("LD_PRELOAD", preload_str)
        .env("FFBTOOLS_LOGGER", "1")
        .env("FFBTOOLS_LOG_FILE", log_file)
        .env("FFBTOOLS_DEVICE_NAME", &device.name)
        .env("FFBTOOLS_DEV_MAJOR", format!("0x{:x}", device.major))
        .env("FFBTOOLS_DEV_MINOR", format!("0x{:x}", device.minor))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    println!("Launching command: {:?}", command_and_args);
    println!("Logging FFB to: {}", log_file.display());

    cmd.spawn()
        .map_err(|e| format!("Failed to spawn command {:?}: {}", command_and_args, e))
}

pub fn create_session_log_path() -> PathBuf {
    let dir = if let Some(proj) = ProjectDirs::from("", "", "ffb-trace") {
        proj.state_dir()
            .unwrap_or_else(|| proj.data_local_dir())
            .to_path_buf()
    } else {
        PathBuf::from("/tmp/ffb-trace")
    };
    let _ = fs::create_dir_all(&dir);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    dir.join(format!("session-{}.log", ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_device_name() {
        assert_eq!(
            clean_device_name("usb-Gudsen_MOZA_R9_Base_36001E-if02-event-joystick"),
            "Gudsen MOZA R9 Base 36001E-if02"
        );
        assert_eq!(
            clean_device_name("usb-Logitech_G29_Driving_Force_Racing_Wheel-event-joystick"),
            "Logitech G29 Driving Force Racing Wheel"
        );
    }

    #[test]
    fn test_mask_serial() {
        let raw = "Gudsen MOZA R9 Base 36001E000651353430333631-if02";
        assert_eq!(mask_serial(raw), "Gudsen MOZA R9 Base ••••••••-if02");

        let normal = "MOZA R9 Base";
        assert_eq!(mask_serial(normal), "MOZA R9 Base");

        let fanatec = "Fanatec ClubSport Wheel Base 00000000001A";
        assert_eq!(
            mask_serial(fanatec),
            "Fanatec ClubSport Wheel Base ••••••••"
        );
    }

    #[test]
    fn test_vendor_priority() {
        assert_eq!(vendor_priority("usb-Gudsen_MOZA_R9_Base-event-joystick"), 0);
        assert_eq!(vendor_priority("usb-MOZA_R5-event-joystick"), 0);
        assert_eq!(vendor_priority("usb-Fanatec_ClubSport-event-joystick"), 1);
        assert_eq!(vendor_priority("usb-Logitech_G923-event-joystick"), 1);
        assert_eq!(vendor_priority("usb-Generic_Gamepad-event-joystick"), 2);
    }

    #[test]
    fn test_create_session_log_path() {
        let path = create_session_log_path();
        assert!(path.to_string_lossy().contains("session-"));
        assert!(path.to_string_lossy().ends_with(".log"));
    }

    #[test]
    fn test_ensure_preload_libraries() {
        let res = ensure_preload_libraries();
        assert!(res.is_ok(), "Should extract embedded preload libraries");
        let (x86_64, i386) = res.unwrap();
        assert!(x86_64.exists());
        assert!(x86_64
            .to_string_lossy()
            .ends_with("libffbwrapper-x86_64.so"));
        if let Some(i386_path) = i386 {
            assert!(i386_path.exists());
        }
    }
}
