//! Binary entrypoint for the e-voice daemon and client commands.

use clap::{Parser, Subcommand};
use config::AppConfig;
use daemon::{Daemon, Request, Response, socket_path};
use diagnostics::CheckStatus;
use serde_json::json;
use std::io::IsTerminal;
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;
use transport::DaemonTransport;

mod config;
mod daemon;
mod diagnostics;
mod http_server;
mod modes;
mod processor;
mod setup;
mod system_adapter;
mod transport;
mod tray;

static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Parser, Debug)]
#[command(
    name = "e-voice",
    version,
    about = "LLM-powered dictation post processor",
    arg_required_else_help = true
)]
struct Cli {
    /// Start only the HTTP server (no Tauri UI). Starts e-voice in headless
    /// mode bound to 127.0.0.1 on the configured HTTP port.
    #[arg(long, conflicts_with = "command")]
    headless: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the headless Unix socket daemon (deprecated — use --headless instead)
    Daemon,
    #[command(about = "Start daemon with system tray icon")]
    Tray,
    #[command(
        about = "Process stdin text through the LLM (called by voxtype as a post-process hook)"
    )]
    Process,
    #[command(about = "Show daemon status")]
    Status {
        #[arg(long, help = "Output as JSON for Waybar integration")]
        format: Option<String>,
        #[arg(long, help = "Poll continuously and print on mode changes")]
        follow: bool,
    },
    #[command(about = "First-run setup wizard")]
    Setup,
    #[command(about = "Diagnose pipeline health")]
    Doctor,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_logging(cli.headless, cli.command.as_ref());

    if let Err(err) = run(cli).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    if cli.headless {
        return run_http_server().await;
    }

    match cli.command {
        Some(Commands::Daemon) => run_headless_daemon().await,
        Some(Commands::Tray) => run_tray_daemon().await,
        Some(Commands::Process) => {
            if io::stdin().is_terminal() {
                return Err(
                    "Pipe text via stdin, e.g.: echo 'hello' | e-voice process\n\
                     The daemon must be running: e-voice daemon"
                        .to_owned(),
                );
            }

            let mut input = String::new();
            io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| format!("failed to read stdin: {e}"))?;
            let text = input.trim_end_matches('\n').to_owned();

            if text.trim().is_empty() {
                return Ok(());
            }

            let request_id = next_request_id();
            info!(request_id = %request_id, input_len = text.len(), "starting process command");

            let client = DaemonTransport::new(
                socket_path().map_err(|e| format!("failed to resolve socket path: {e}"))?,
            );
            match client
                .send(Request::Process {
                    text: text.clone(),
                    request_id: Some(request_id.clone()),
                    profile: None,
                })
                .await
            {
                Ok(Response::Text(output)) => {
                    info!(request_id = %request_id, output_len = output.len(), "process command completed with daemon output");
                    print!("{output}");
                    Ok(())
                }
                Ok(Response::Error(err)) => {
                    eprintln!("e-voice: daemon error: {err}");
                    print!("{text}");
                    Err("process failed with daemon error".to_owned())
                }
                Ok(other) => {
                    eprintln!("e-voice: unexpected daemon response, falling back to raw input");
                    print!("{text}");
                    Err(format!("process failed: unexpected response {other:?}"))
                }
                Err(err) => {
                    eprintln!("e-voice: daemon unreachable ({err}). Start it with: e-voice daemon");
                    print!("{text}");
                    Err("process failed: daemon unreachable".to_owned())
                }
            }
        }
        Some(Commands::Status { format, follow }) => {
            let as_json = format.as_deref() == Some("json");
            print_status(as_json, follow).await
        }
        Some(Commands::Setup) => {
            let adapter = system_adapter::LinuxSystemAdapter;
            setup::run_setup(&adapter).map_err(|e| e.to_string())
        }
        Some(Commands::Doctor) => run_doctor().await,
        None => unreachable!("clap guarantees either --headless or a subcommand is provided"),
    }
}

async fn run_headless_daemon() -> Result<(), String> {
    let config = AppConfig::load().map_err(|e| format!("failed to load config: {e}"))?;
    let daemon = Daemon::new(config).map_err(|e| format!("failed to initialize daemon: {e}"))?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let signal_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async {
                    if let Some(sig) = sigterm.as_mut() {
                        let _ = sig.recv().await;
                    }
                } => {}
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }

        let _ = signal_tx.send(true);
    });

    daemon
        .run(shutdown_rx)
        .await
        .map_err(|e| format!("daemon failed: {e}"))
}

async fn run_http_server() -> Result<(), String> {
    let config = AppConfig::load().map_err(|e| format!("failed to load config: {e}"))?;
    let port = http_port_from_env().unwrap_or(http_server::DEFAULT_HTTP_PORT);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let signal_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async {
                    if let Some(sig) = sigterm.as_mut() {
                        let _ = sig.recv().await;
                    }
                } => {}
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }

        let _ = signal_tx.send(true);
    });

    http_server::run(config, port, shutdown_rx)
        .await
        .map_err(|e| format!("HTTP server failed: {e}"))
}

fn http_port_from_env() -> Option<u16> {
    std::env::var("E_VOICE_HTTP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
}

async fn run_tray_daemon() -> Result<(), String> {
    let adapter = system_adapter::LinuxSystemAdapter;
    tray::run(&adapter).await.map_err(|e| e.to_string())
}

fn init_logging(headless: bool, command: Option<&Commands>) {
    let default_filter = if headless {
        "e_voice=info"
    } else {
        match command {
            Some(Commands::Daemon | Commands::Tray) => "e_voice=info",
            Some(Commands::Process) => "off",
            Some(Commands::Status { .. } | Commands::Setup | Commands::Doctor) | None => "off",
        }
    };

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init();
}

fn next_request_id() -> String {
    let seq = REQUEST_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("req-{seq}")
}

async fn print_status(as_json: bool, follow: bool) -> Result<(), String> {
    let client = DaemonTransport::new(
        socket_path().map_err(|e| format!("failed to resolve socket path: {e}"))?,
    );
    let mut last_printed = false;

    loop {
        let response = client
            .send(Request::GetStatus)
            .await
            .map_err(|e| format!("failed to query daemon status: {e}"))?;

        match response {
            Response::Status { .. } => {}
            Response::Error(err) => return Err(format!("daemon error: {err}")),
            other => return Err(format!("unexpected response: {other:?}")),
        }

        if !last_printed || follow {
            if as_json {
                let payload = json!({
                    "text": "active",
                    "class": "active",
                    "tooltip": "e-voice active",
                });
                println!("{payload}");
            } else {
                println!("active");
            }
            last_printed = true;
        }

        if !follow {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}

async fn run_doctor() -> Result<(), String> {
    println!("e-voice doctor");

    let config = match AppConfig::load() {
        Ok(cfg) => {
            println!("[ok] config load");
            cfg
        }
        Err(err) => {
            println!("[fail] config load: {err}");
            return Err("doctor failed: config could not be loaded".to_owned());
        }
    };

    let sock =
        socket_path().map_err(|e| format!("doctor failed: cannot resolve socket path: {e}"))?;
    let client = DaemonTransport::new(sock.clone());
    let adapter = system_adapter::LinuxSystemAdapter;
    let results = diagnostics::run(&config, &client, sock, &adapter).await;

    let mut failures = 0usize;
    for result in results {
        if result.status == CheckStatus::Fail {
            failures += 1;
        }
        println!(
            "[{}] {}",
            render_check_status(result.status),
            result.message
        );
    }

    if failures == 0 {
        println!("[ok] doctor completed successfully");
        Ok(())
    } else {
        Err(format!("doctor found {failures} issue(s)"))
    }
}

fn render_check_status(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "ok",
        CheckStatus::Fail => "fail",
        CheckStatus::Info => "info",
        CheckStatus::Warn => "warn",
    }
}

