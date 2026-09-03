//! Log and stream source ingestion.
//!
//! Supports reading from a specified file, standard input (or FIFO), or auto-detecting
//! the newest log in the XDG state directory (~/.local/state/ffb-trace/ or ~/ffblogs/).

use parking_lot::RwLock;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::parser::parse_line;
use crate::tracker::FfbTracker;

#[derive(Debug, Clone)]
pub enum SourceMode {
    File(PathBuf),
    Directories(Vec<PathBuf>),
    Stdin,
}

pub struct LogSource {
    mode: SourceMode,
    tracker: Arc<RwLock<FfbTracker>>,
    current_source_description: Arc<RwLock<String>>,
    follow: bool,
    stop_flag: Arc<AtomicBool>,
}

impl LogSource {
    pub fn new(
        mode: SourceMode,
        tracker: Arc<RwLock<FfbTracker>>,
        follow: bool,
    ) -> (Self, Arc<RwLock<String>>, Arc<AtomicBool>) {
        let desc = Arc::new(RwLock::new("Initializing source...".to_string()));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let source = Self {
            mode,
            tracker,
            current_source_description: desc.clone(),
            follow,
            stop_flag: stop_flag.clone(),
        };
        (source, desc, stop_flag)
    }

    pub fn run(self) {
        match &self.mode {
            SourceMode::Stdin => self.run_stdin(),
            SourceMode::File(path) => self.run_file(path),
            SourceMode::Directories(dirs) => self.run_directories(dirs),
        }
    }

    fn run_stdin(&self) {
        *self.current_source_description.write() = "Reading from stdin / pipe".to_string();
        let stdin = std::io::stdin();
        let reader = stdin.lock();
        for line in reader.lines() {
            if self.stop_flag.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(l) = line {
                if let Some(event) = parse_line(&l) {
                    self.tracker.write().process_event(event);
                }
            }
        }
    }

    fn run_file(&self, path: &Path) {
        *self.current_source_description.write() = format!("Reading file: {}", path.display());
        let Ok(file) = File::open(path) else {
            *self.current_source_description.write() =
                format!("Failed to open: {}", path.display());
            return;
        };

        let mut reader = BufReader::new(file);
        let mut line_buf = String::new();

        loop {
            if self.stop_flag.load(Ordering::Relaxed) {
                break;
            }
            line_buf.clear();
            match reader.read_line(&mut line_buf) {
                Ok(0) => {
                    if self.follow {
                        std::thread::sleep(Duration::from_millis(10));
                    } else {
                        *self.current_source_description.write() =
                            format!("Finished: {}", path.display());
                        break;
                    }
                }
                Ok(_) => {
                    if let Some(event) = parse_line(&line_buf) {
                        self.tracker.write().process_event(event);
                    }
                }
                Err(e) => {
                    *self.current_source_description.write() = format!("Read error: {}", e);
                    break;
                }
            }
        }
    }

    fn run_directories(&self, dirs: &[PathBuf]) {
        let mut current_tracked_file: Option<PathBuf> = None;
        let mut reader: Option<BufReader<File>> = None;
        let mut line_buf = String::new();

        while !self.stop_flag.load(Ordering::Relaxed) {
            // Check for the newest log file across all candidate directories
            let newest = find_newest_log_in_dirs(dirs);

            if let Some(ref path) = newest {
                let is_new_session = match &current_tracked_file {
                    Some(cur) => cur != path,
                    None => true,
                };

                if is_new_session {
                    if let Ok(file) = File::open(path) {
                        current_tracked_file = Some(path.clone());
                        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                        *self.current_source_description.write() =
                            format!("Tracking: {}", file_name);

                        // Reset session on new file
                        self.tracker.write().reset_session();
                        reader = Some(BufReader::new(file));
                    }
                }
            } else {
                let dirs_display: Vec<String> =
                    dirs.iter().map(|d| d.display().to_string()).collect();
                *self.current_source_description.write() = format!(
                    "Watching: {} (waiting for logs...)",
                    dirs_display.join(", ")
                );
            }

            if let Some(ref mut r) = reader {
                let mut read_anything = false;
                // Read available lines in batch (up to 500 lines per tick)
                for _ in 0..500 {
                    line_buf.clear();
                    match r.read_line(&mut line_buf) {
                        Ok(0) => break,
                        Ok(_) => {
                            read_anything = true;
                            if let Some(event) = parse_line(&line_buf) {
                                self.tracker.write().process_event(event);
                            }
                        }
                        Err(_) => break,
                    }
                }

                if !read_anything {
                    std::thread::sleep(Duration::from_millis(15));
                }
            } else {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

#[allow(dead_code)]
pub fn find_newest_log(dir: &Path) -> Option<PathBuf> {
    find_newest_log_in_dirs(&[dir.to_path_buf()])
}

pub fn find_newest_log_in_dirs(dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "log") {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if best
                                .as_ref()
                                .is_none_or(|(best_time, _)| mtime > *best_time)
                            {
                                best = Some((mtime, path));
                            }
                        }
                    }
                }
            }
        }
    }

    best.map(|(_, path)| path)
}
