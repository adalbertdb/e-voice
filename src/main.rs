//! Binary entrypoint for the e-voice daemon and client commands.

use clap::{Parser, Subcommand};
use client::DaemonClient;
use config::AppConfig;
use daemon::{Daemon, Request, Response, socket_path};
use reqwest::StatusCode;
use serde_json::json;
use std::collections::BTreeSet;
use std::io::{self, Read};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod client;
mod config;
mod daemon;
mod modes;
mod processor;
mod setup;
mod tray;

static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Parser, Debug)]
#[command(
    name = "e-voice",
    version,
    about = "LLM-powered dictation post processor"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Daemon,
    Tray,
    Process,
    Mode {
        mode: String,
    },
    Status {
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        follow: bool,
    },
    Setup,
    Menu,
    Doctor,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_logging(&cli.command);

    if let Err(err) = run(cli).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Daemon => run_headless_daemon().await,
        Commands::Tray => run_tray_daemon(),
        Commands::Process => {
            let mut input = String::new();
            io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| format!("failed to read stdin: {e}"))?;
            let text = input.trim_end_matches('\n').to_owned();
            let request_id = next_request_id();
            info!(request_id = %request_id, input_len = text.len(), "starting process command");

            let client = DaemonClient::new();
            match client
                .send(Request::Process {
                    text: text.clone(),
                    request_id: Some(request_id.clone()),
                })
                .await
            {
                Ok(Response::Text(output)) => {
                    info!(request_id = %request_id, output_len = output.len(), "process command completed with daemon output");
                    print!("{output}");
                }
                Ok(Response::Error(_)) | Ok(_) | Err(_) => {
                    warn!(request_id = %request_id, "process command fell back to raw input");
                    print!("{input}");
                }
            }
            Ok(())
        }
        Commands::Mode { mode } => {
            let client = DaemonClient::new();
            match client.send(Request::SetMode { mode: mode.clone() }).await {
                Ok(Response::ModeChanged { mode }) => {
                    println!("{mode}");
                    Ok(())
                }
                Ok(Response::Error(err)) => Err(format!("daemon error: {err}")),
                Ok(other) => Err(format!("unexpected response: {other:?}")),
                Err(err) => Err(format!("failed to reach daemon: {err}")),
            }
        }
        Commands::Status { format, follow } => {
            let as_json = format.as_deref() == Some("json");
            print_status(as_json, follow).await
        }
        Commands::Setup => setup::run_setup().map_err(|e| e.to_string()),
        Commands::Menu => {
            println!("{}", menu_json());
            Ok(())
        }
        Commands::Doctor => run_doctor().await,
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
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

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

fn run_tray_daemon() -> Result<(), String> {
    let runtime = tray::start_daemon_runtime().map_err(|e| e.to_string())?;
    tray::run_tray_loop(runtime).map_err(|e| e.to_string())
}

fn init_logging(command: &Commands) {
    let default_filter = match command {
        Commands::Daemon | Commands::Tray => "e_voice=info",
        Commands::Process => "off",
        Commands::Mode { .. }
        | Commands::Status { .. }
        | Commands::Setup
        | Commands::Menu
        | Commands::Doctor => "off",
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
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
    let client = DaemonClient::new();
    let mut last_mode: Option<String> = None;

    loop {
        let response = client
            .send(Request::GetStatus)
            .await
            .map_err(|e| format!("failed to query daemon status: {e}"))?;

        let mode = match response {
            Response::Status { mode, .. } => mode,
            Response::Error(err) => return Err(format!("daemon error: {err}")),
            other => return Err(format!("unexpected response: {other:?}")),
        };

        if last_mode.as_deref() != Some(mode.as_str()) {
            if as_json {
                let payload = json!({
                    "text": mode.clone(),
                    "class": mode.clone(),
                    "tooltip": format!("e-voice active | mode: {}", mode),
                });
                println!("{payload}");
            } else {
                println!("{mode}");
            }
            last_mode = Some(mode);
        }

        if !follow {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}

fn menu_json() -> String {
    json!([
        {"label": "Clean (remove fillers)", "value": "clean", "exec": "e-voice mode clean"},
        {"label": "Formal (professional tone)", "value": "formal", "exec": "e-voice mode formal"},
        {"label": "Casual (relaxed tone)", "value": "casual", "exec": "e-voice mode casual"},
        {"label": "Bullet (format as list)", "value": "bullet", "exec": "e-voice mode bullet"},
        {"label": "Translate to Spanish", "value": "translate:es", "exec": "e-voice mode translate:es"},
        {"label": "Translate to Portuguese", "value": "translate:pt", "exec": "e-voice mode translate:pt"},
        {"label": "Translate to French", "value": "translate:fr", "exec": "e-voice mode translate:fr"}
    ])
    .to_string()
}

#[derive(Debug, serde::Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelTag>,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaModelTag {
    name: String,
}

async fn run_doctor() -> Result<(), String> {
    let mut failures = 0usize;

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

    let configured_models = configured_models(&config);
    println!("[info] ollama url: {}", config.ollama.url);
    println!("[info] configured models: {}", configured_models.join(", "));

    let sock = socket_path().map_err(|e| format!("doctor failed: cannot resolve socket path: {e}"))?;
    if sock.exists() {
        println!("[ok] daemon socket exists: {}", sock.display());
    } else {
        failures += 1;
        println!("[fail] daemon socket missing: {}", sock.display());
    }

    let client = DaemonClient::new();
    match client.send(Request::GetStatus).await {
        Ok(Response::Status { mode, version }) => {
            println!("[ok] daemon reachable: mode={mode} version={version}");
        }
        Ok(Response::Error(err)) => {
            failures += 1;
            println!("[fail] daemon error response: {err}");
        }
        Ok(other) => {
            failures += 1;
            println!("[fail] unexpected daemon response: {other:?}");
        }
        Err(err) => {
            failures += 1;
            println!("[fail] daemon unreachable: {err}");
        }
    }

    let tags_url = format!("{}/api/tags", config.ollama.url.trim_end_matches('/'));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("doctor failed: cannot initialize HTTP client: {e}"))?;

    match http.get(&tags_url).send().await {
        Ok(resp) if resp.status() == StatusCode::OK => {
            match resp.json::<OllamaTagsResponse>().await {
                Ok(body) => {
                    let available: BTreeSet<String> = body.models.into_iter().map(|m| m.name).collect();
                    println!("[ok] ollama reachable: {} models available", available.len());

                    let mut missing = Vec::new();
                    for model in &configured_models {
                        if !available.contains(model) {
                            missing.push(model.clone());
                        }
                    }

                    if missing.is_empty() {
                        println!("[ok] configured models available in ollama");
                    } else {
                        failures += 1;
                        println!("[fail] missing models in ollama: {}", missing.join(", "));
                    }
                }
                Err(err) => {
                    failures += 1;
                    println!("[fail] ollama tags parse failed: {err}");
                }
            }
        }
        Ok(resp) => {
            failures += 1;
            println!("[fail] ollama tags endpoint returned status {}", resp.status());
        }
        Err(err) => {
            failures += 1;
            println!("[fail] ollama unreachable: {err}");
        }
    }

    if failures == 0 {
        println!("[ok] doctor completed successfully");
        Ok(())
    } else {
        Err(format!("doctor found {failures} issue(s)"))
    }
}

fn configured_models(config: &AppConfig) -> Vec<String> {
    let mut models = BTreeSet::new();
    models.insert(config.ollama.model.clone());
    models.insert(config.models.clean.clone());
    models.insert(config.models.formal.clone());
    models.insert(config.models.casual.clone());
    models.insert(config.models.bullet.clone());
    models.insert(config.models.translate.clone());
    models.into_iter().collect()
}
