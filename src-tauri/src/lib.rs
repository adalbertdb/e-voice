//! e-voice Tauri application library.
//!
//! Entry-point for the desktop app. Parses `--headless` before Tauri is
//! initialized so the flag can be used in environments without a display.

use clap::Parser;
use tracing::info;

#[cfg(not(test))]
mod tray;

/// CLI arguments for the e-voice Tauri application.
#[derive(Parser, Debug)]
#[command(
    name = "e-voice",
    version,
    about = "LLM-powered dictation post-processor (desktop app)"
)]
pub struct AppArgs {
    /// Skip the GUI and run as a background HTTP server only.
    ///
    /// Useful for headless servers, CI environments, or when you want to drive
    /// e-voice from another frontend. Logs the listening port to stdout.
    #[arg(long, help = "Start without the GUI window; run background services only")]
    pub headless: bool,

    /// Port for the background HTTP server (only used with --headless).
    #[arg(long, default_value = "4242", help = "HTTP server port (headless mode)")]
    pub port: u16,
}

/// Application run mode derived from parsed arguments.
#[derive(Debug, PartialEq)]
pub enum RunMode {
    /// Run the full Tauri desktop app with window and system tray.
    Desktop,
    /// Run only the background HTTP server, no GUI.
    Headless { port: u16 },
}

impl RunMode {
    /// Determine the run mode from CLI arguments.
    pub fn from_args(args: &AppArgs) -> Self {
        if args.headless {
            RunMode::Headless { port: args.port }
        } else {
            RunMode::Desktop
        }
    }
}

/// Start the application in headless mode (no Tauri, HTTP server only).
pub fn run_headless(port: u16) {
    info!("headless mode, HTTP server on {port}");
    println!("headless mode, HTTP server on {port}");
    // HTTP server will be wired up in a future slice (issue #10).
    // For now we park the thread so process lifetime is observable.
    std::thread::park();
}

/// Start the full Tauri desktop application.
#[cfg(not(test))]
pub fn run_tauri() {
    tray::build_app()
        .run(tauri::generate_context!())
        .expect("error running e-voice Tauri app");
}

/// Public entry-point called from `main.rs`.
pub fn run() {
    init_logging();
    let args = AppArgs::parse();
    match RunMode::from_args(&args) {
        RunMode::Headless { port } => run_headless(port),
        RunMode::Desktop => {
            #[cfg(not(test))]
            run_tauri();
            #[cfg(test)]
            unreachable!("run() should not be called in test mode");
        }
    }
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("e_voice_app_lib=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Headless flag tests ───────────────────────────────────────────────────

    /// `--headless` must resolve to `RunMode::Headless`, NOT `RunMode::Desktop`.
    /// This guarantees Tauri is never initialized in headless mode.
    #[test]
    fn test_headless_flag_no_tauri() {
        let args = AppArgs {
            headless: true,
            port: 4242,
        };
        let mode = RunMode::from_args(&args);
        assert_eq!(
            mode,
            RunMode::Headless { port: 4242 },
            "--headless must produce RunMode::Headless, not Desktop"
        );
        // Confirm it is explicitly NOT the Desktop variant.
        assert_ne!(mode, RunMode::Desktop);
    }

    /// Without `--headless`, the mode must be `RunMode::Desktop`, which is the
    /// code-path that initialises the Tauri application.
    #[test]
    fn test_default_mode_starts_tauri() {
        let args = AppArgs {
            headless: false,
            port: 4242,
        };
        let mode = RunMode::from_args(&args);
        assert_eq!(
            mode,
            RunMode::Desktop,
            "default (no --headless) must produce RunMode::Desktop"
        );
    }

    /// Custom port is preserved in headless mode.
    #[test]
    fn test_headless_custom_port() {
        let args = AppArgs {
            headless: true,
            port: 9090,
        };
        let mode = RunMode::from_args(&args);
        assert_eq!(mode, RunMode::Headless { port: 9090 });
    }

    /// `RunMode::Desktop` and `RunMode::Headless` are distinct variants.
    #[test]
    fn test_run_modes_are_distinct() {
        assert_ne!(RunMode::Desktop, RunMode::Headless { port: 4242 });
    }
}
