mod parser;
mod runner;
mod source;
mod spectrum;
mod tracker;
mod ui;

use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use source::{LogSource, SourceMode};
use tracker::FfbTracker;
use ui::FfbTraceApp;

#[derive(Parser, Debug)]
#[command(
    name = "ffb-trace",
    version,
    about = "Real-time Force Feedback (FFB) clipping and telemetry monitor for Linux sim racing"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Read a specific ffbwrap log file directly
    #[arg(short, long)]
    file: Option<PathBuf>,

    /// Watch a directory for the latest .log file
    #[arg(short, long)]
    log_dir: Option<PathBuf>,

    /// Read from standard input or named pipe (FIFO)
    #[arg(long)]
    stdin: bool,

    /// Run in headless CLI mode without opening GUI window
    #[arg(long)]
    no_gui: bool,

    /// Start in compact Mini-HUD overlay mode
    #[arg(long)]
    mini: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Launch a game on-demand with automatic wheel detection and FFB tracing
    Run {
        /// Explicit wheel device path (optional, auto-detected from /dev/input/by-id/ if omitted)
        #[arg(long)]
        device: Option<PathBuf>,

        /// Run without opening GUI window
        #[arg(long)]
        no_gui: bool,

        /// Start in compact Mini-HUD overlay mode
        #[arg(long)]
        mini: bool,

        /// Command and arguments to run (e.g. steam steam://rungameid/378860 or %command%)
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    let tracker = Arc::new(RwLock::new(FfbTracker::default()));

    let is_headless = args.no_gui
        || match &args.command {
            Some(Commands::Run { no_gui, .. }) => *no_gui,
            None => false,
        };

    // Determine input mode
    let mode = if let Some(Commands::Run {
        ref device,
        ref command,
        ..
    }) = args.command
    {
        let dev = match runner::detect_wheel_device(device.as_deref()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error detecting steering wheel: {}", e);
                std::process::exit(1);
            }
        };
        let log_path = runner::create_session_log_path();
        if let Err(e) = runner::run_preloaded_command(command, &dev, &log_path) {
            eprintln!("Error launching game: {}", e);
            std::process::exit(1);
        }
        SourceMode::File(log_path)
    } else if args.stdin {
        SourceMode::Stdin
    } else if let Some(path) = args.file {
        SourceMode::File(path)
    } else {
        let candidate_dirs = if let Some(dir) = args.log_dir {
            vec![dir]
        } else {
            let mut dirs = Vec::new();
            if let Some(proj_dirs) = ProjectDirs::from("", "", "ffb-trace") {
                let state_dir = proj_dirs
                    .state_dir()
                    .unwrap_or_else(|| proj_dirs.data_local_dir());
                let _ = std::fs::create_dir_all(state_dir);
                dirs.push(state_dir.to_path_buf());
            }
            if let Some(home) = std::env::var_os("HOME") {
                let ffblogs = PathBuf::from(home).join("ffblogs");
                if ffblogs.exists() {
                    dirs.push(ffblogs);
                }
            }
            dirs
        };

        SourceMode::Directories(candidate_dirs)
    };

    // Spawn source ingestion thread
    let (source, source_description, _stop_flag) = LogSource::new(mode, tracker.clone(), true);
    std::thread::Builder::new()
        .name("ffb-source-reader".to_string())
        .spawn(move || {
            source.run();
        })?;

    let start_mini = args.mini
        || match &args.command {
            Some(Commands::Run { mini, .. }) => *mini,
            _ => false,
        };

    if is_headless {
        println!("ffb-trace running in headless mode.");
        println!("Press Ctrl+C to stop.");
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let status = tracker.read().format_status_line();
            println!("{}", status);
        }
    }

    // Launch egui Desktop UI
    let (initial_w, initial_h) = if start_mini {
        (440.0, 96.0)
    } else {
        (880.0, 680.0)
    };
    let (min_w, min_h) = if start_mini {
        (320.0, 80.0)
    } else {
        (560.0, 420.0)
    };

    let mut builder = eframe::egui::ViewportBuilder::default()
        .with_inner_size([initial_w, initial_h])
        .with_min_inner_size([min_w, min_h])
        .with_title("ffb-trace — FFB Monitor");

    if start_mini {
        builder = builder.with_always_on_top();
    }

    let native_options = eframe::NativeOptions {
        viewport: builder,
        ..Default::default()
    };

    eframe::run_native(
        "ffb-trace",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(FfbTraceApp::new(
                tracker,
                source_description,
                start_mini,
            )))
        }),
    )?;

    Ok(())
}
