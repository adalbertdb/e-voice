//! System tray for STT language and LLM model switching via ksni (StatusNotifierItem DBus).

use crate::config::{AppConfig, config_file_path};
use crate::daemon::{Daemon, Request, Response, socket_path};
use crate::transport::{DaemonTransport, DaemonTransportError};
use ksni::TrayMethods;
use std::process::Command;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

const STT_PRESETS: [(&str, &str, &str); 6] = [
    ("English", "base.en", "en"),
    ("Spanish", "base", "es"),
    ("French", "base", "fr"),
    ("Portuguese", "base", "pt"),
    ("German", "base", "de"),
    ("Italian", "base", "it"),
];

#[derive(Debug)]
enum TrayCmd {
    SetSttLanguage { model: String, language: String },
    SetOllamaModel(String),
    OpenConfig,
    Shutdown,
}

#[derive(Debug, Clone)]
struct HealthInfo {
    ollama_reachable: bool,
    voxtype_running: bool,
}

struct TrayState {
    stt_label: String,
    stt_model: String,
    stt_language: String,
    ollama_model: String,
    ollama_models: Vec<String>,
    health: HealthInfo,
    control_tx: mpsc::UnboundedSender<TrayCmd>,
}

impl ksni::Tray for TrayState {
    fn id(&self) -> String {
        "e-voice".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![build_icon(match &self.health {
            HealthInfo {
                ollama_reachable: true,
                voxtype_running: true,
            } => [0x4c, 0xaf, 0x50, 0xff],
            HealthInfo {
                ollama_reachable: false,
                ..
            } => [0xff, 0x98, 0x00, 0xff],
            _ => [0xf4, 0x43, 0x36, 0xff],
        })]
    }

    fn title(&self) -> String {
        format!("e-voice: {}", self.stt_label)
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "e-voice".into(),
            description: format!(
                "STT: {} ({})\nLLM: {}\nOllama: {}\nVoxtype: {}",
                self.stt_label,
                self.stt_model,
                self.ollama_model,
                if self.health.ollama_reachable {
                    "connected"
                } else {
                    "unreachable"
                },
                if self.health.voxtype_running {
                    "running"
                } else {
                    "stopped"
                },
            ),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;

        let stt_selected = STT_PRESETS
            .iter()
            .position(|(_, m, l)| *m == self.stt_model && *l == self.stt_language)
            .unwrap_or(0);

        let ollama_selected = self
            .ollama_models
            .iter()
            .position(|m| m == &self.ollama_model)
            .unwrap_or(0);

        let stt_options: Vec<RadioItem> = STT_PRESETS
            .iter()
            .map(|(label, _, _)| RadioItem {
                label: label.to_string(),
                ..Default::default()
            })
            .collect();

        let ollama_options: Vec<RadioItem> = self
            .ollama_models
            .iter()
            .map(|m| RadioItem {
                label: m.clone(),
                ..Default::default()
            })
            .collect();

        let ollama_submenu_item = if ollama_options.is_empty() {
            StandardItem {
                label: "(no models found)".into(),
                enabled: false,
                ..Default::default()
            }
            .into()
        } else {
            RadioGroup {
                selected: ollama_selected,
                select: Box::new(|this: &mut Self, idx| {
                    if let Some(model) = this.ollama_models.get(idx) {
                        let _ = this.control_tx.send(TrayCmd::SetOllamaModel(model.clone()));
                    }
                }),
                options: ollama_options,
            }
            .into()
        };

        vec![
            SubMenu {
                label: "STT Language".into(),
                submenu: vec![
                    RadioGroup {
                        selected: stt_selected,
                        select: Box::new(|this: &mut Self, idx| {
                            if let Some((_, model, lang)) = STT_PRESETS.get(idx) {
                                let _ = this.control_tx.send(TrayCmd::SetSttLanguage {
                                    model: model.to_string(),
                                    language: lang.to_string(),
                                });
                            }
                        }),
                        options: stt_options,
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Ollama Model".into(),
                submenu: vec![ollama_submenu_item],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Open Config".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.control_tx.send(TrayCmd::OpenConfig);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.control_tx.send(TrayCmd::Shutdown);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn build_icon(rgba: [u8; 4]) -> ksni::Icon {
    let size = 24;
    let mut data = Vec::with_capacity(size * size * 4);
    for _ in 0..(size * size) {
        data.extend_from_slice(&rgba);
    }
    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

pub async fn run() -> Result<(), TrayError> {
    let config = AppConfig::load().map_err(|e| TrayError::Config(e.to_string()))?;
    let ollama_url = config.ollama.url.clone();
    let default_model = config.ollama.model.clone();

    let daemon = Daemon::new(config).map_err(|e| TrayError::Daemon(e.to_string()))?;

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let daemon_task = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move { daemon.run(shutdown_rx).await })
    };

    let signal_task = {
        let shutdown_tx = shutdown_tx.clone();
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
            let _ = shutdown_tx.send(true);
        })
    };

    let (stt_label, stt_model, stt_language) = read_voxtype_stt_config();
    let initial_health = check_health(&ollama_url);
    let ollama_models = fetch_ollama_models(&ollama_url).await.unwrap_or_default();

    let (control_tx, mut control_rx) = mpsc::unbounded_channel();

    let tray_state = TrayState {
        stt_label,
        stt_model,
        stt_language,
        ollama_model: default_model,
        ollama_models,
        health: initial_health,
        control_tx: control_tx.clone(),
    };

    let tray_handle = match tray_state.spawn().await {
        Ok(h) => h,
        Err(err) => {
            let _ = shutdown_tx.send(true);
            return Err(TrayError::TraySpawn(format!("{err}")));
        }
    };

    info!("tray icon spawned, entering command loop");

    let mut health_tick = tokio::time::interval(Duration::from_secs(15));

    loop {
        tokio::select! {
            cmd = control_rx.recv() => {
                match cmd {
                    Some(TrayCmd::SetSttLanguage { model, language }) => {
                        let label = STT_PRESETS.iter()
                            .find(|(_, m, l)| *m == model && *l == language)
                            .map(|(label, _, _)| label.to_string())
                            .unwrap_or_else(|| language.clone());

                        let model_clone = model.clone();
                        let lang_clone = language.clone();
                        let label_clone = label.clone();

                        match write_voxtype_stt_config(&model, &language) {
                            Ok(()) => {
                                info!(model = %model, language = %language, "voxtype STT config updated");
                                let _ = Command::new("systemctl")
                                    .args(["--user", "restart", "voxtype.service"])
                                    .status();
                                info!("voxtype service restarted");
                                tray_handle.update(|tray: &mut TrayState| {
                                    tray.stt_label = label_clone;
                                    tray.stt_model = model_clone;
                                    tray.stt_language = lang_clone;
                                }).await;
                                send_notification(
                                    "STT Language",
                                    &format!("Switched to {} ({})", label, model),
                                );
                            }
                            Err(err) => {
                                error!(%err, "failed to write voxtype config");
                            }
                        }
                    }
                    Some(TrayCmd::SetOllamaModel(model)) => {
                        let transport_result = socket_path()
                            .map_err(|e| TrayError::Daemon(e.to_string()))
                            .map(DaemonTransport::new);
                        match transport_result {
                            Ok(transport) => {
                                match transport.send(Request::SetModel { model: model.clone() }).await {
                                    Ok(Response::ModelChanged { .. }) => {
                                        info!(model = %model, "ollama model changed");
                                        tray_handle.update(|tray: &mut TrayState| {
                                            tray.ollama_model = model.clone();
                                        }).await;
                                        send_notification(
                                            "Ollama Model",
                                            &format!("Switched to {}", model),
                                        );
                                    }
                                    Ok(other) => {
                                        warn!(response = ?other, "unexpected daemon response to SetModel");
                                    }
                                    Err(err) => {
                                        error!(%err, "failed to set ollama model via daemon");
                                    }
                                }
                            }
                            Err(err) => {
                                error!(%err, "failed to resolve daemon socket path");
                            }
                        }
                    }
                    Some(TrayCmd::OpenConfig) => {
                        if let Ok(path) = config_file_path() {
                            let _ = Command::new("xdg-open")
                                .arg(path)
                                .status();
                        }
                    }
                    Some(TrayCmd::Shutdown) => {
                        let _ = shutdown_tx.send(true);
                        tray_handle.shutdown().await;
                        break;
                    }
                    None => break,
                }
            }
            _ = health_tick.tick() => {
                let health = check_health(&ollama_url);
                let models = fetch_ollama_models(&ollama_url).await.unwrap_or_default();
                let _ = tray_handle.update(|tray: &mut TrayState| {
                    tray.health = health;
                    if !models.is_empty() {
                        tray.ollama_models = models;
                    }
                }).await;
            }
            _ = shutdown_rx.changed() => {
                tray_handle.shutdown().await;
                break;
            }
        }
    }

    let _ = signal_task.await;
    let daemon_result = daemon_task.await;
    if let Err(err) = daemon_result {
        error!(%err, "daemon task panicked");
    }

    Ok(())
}

fn read_voxtype_stt_config() -> (String, String, String) {
    let path = voxtype_config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ("Unknown".into(), "base.en".into(), "en".into()),
    };

    let model =
        extract_toml_value_in_whisper(&content, "model").unwrap_or_else(|| "base.en".into());
    let language =
        extract_toml_value_in_whisper(&content, "language").unwrap_or_else(|| "en".into());

    let label = STT_PRESETS
        .iter()
        .find(|(_, m, l)| *m == model && *l == language)
        .map(|(label, _, _)| label.to_string())
        .unwrap_or_else(|| language.clone());

    (label, model, language)
}

fn extract_toml_value_in_whisper(content: &str, key: &str) -> Option<String> {
    let mut in_whisper = false;
    let prefix = format!("{key} = \"");
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_whisper = trimmed == "[whisper]";
            continue;
        }

        if in_whisper
            && trimmed.starts_with(&prefix)
            && let Some(start) = trimmed.find('"')
            && let Some(end) = trimmed[start + 1..].find('"')
        {
            return Some(trimmed[start + 1..start + 1 + end].to_owned());
        }
    }
    None
}

fn write_voxtype_stt_config(model: &str, language: &str) -> Result<(), TrayError> {
    let path = voxtype_config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();

    let mut new_lines: Vec<String> = Vec::new();
    let mut in_whisper = false;
    let mut model_set = false;
    let mut language_set = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_whisper {
                if !model_set {
                    new_lines.push(format!("model = \"{model}\""));
                }
                if !language_set {
                    new_lines.push(format!("language = \"{language}\""));
                }
            }
            in_whisper = trimmed == "[whisper]";
            new_lines.push(line.to_owned());
            continue;
        }

        if in_whisper && trimmed.starts_with("model = \"") {
            new_lines.push(format!("model = \"{model}\""));
            model_set = true;
        } else if in_whisper && trimmed.starts_with("language = \"") {
            new_lines.push(format!("language = \"{language}\""));
            language_set = true;
        } else {
            new_lines.push(line.to_owned());
        }
    }

    if in_whisper {
        if !model_set {
            new_lines.push(format!("model = \"{model}\""));
        }
        if !language_set {
            new_lines.push(format!("language = \"{language}\""));
        }
    }

    if !new_lines.iter().any(|l| l.trim() == "[whisper]") {
        if !new_lines.is_empty() {
            new_lines.push(String::new());
        }
        new_lines.push("[whisper]".to_owned());
        new_lines.push(format!("model = \"{model}\""));
        new_lines.push(format!("language = \"{language}\""));
    }

    std::fs::write(&path, new_lines.join("\n") + "\n")?;
    Ok(())
}

fn voxtype_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(home).join(".config/voxtype/config.toml")
}

fn check_health(ollama_url: &str) -> HealthInfo {
    let ollama_reachable = std::process::Command::new("curl")
        .args(["-sf", "--max-time", "2", &format!("{ollama_url}/api/tags")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let voxtype_running = std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "voxtype.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    HealthInfo {
        ollama_reachable,
        voxtype_running,
    }
}

async fn fetch_ollama_models(ollama_url: &str) -> Result<Vec<String>, TrayError> {
    let url = format!("{}/api/tags", ollama_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    #[derive(serde::Deserialize)]
    struct TagsResponse {
        models: Vec<TagEntry>,
    }
    #[derive(serde::Deserialize)]
    struct TagEntry {
        name: String,
    }
    let body: TagsResponse = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(body.models.into_iter().map(|m| m.name).collect())
}

fn send_notification(summary: &str, body: &str) {
    let summary = summary.to_owned();
    let body = body.to_owned();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .icon("e-voice")
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show();
    });
}

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("config error: {0}")]
    Config(String),
    #[error("daemon error: {0}")]
    Daemon(String),
    #[error("tray spawn error: {0}")]
    TraySpawn(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("daemon socket error: {0}")]
    Socket(#[from] crate::daemon::DaemonError),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("transport error: {0}")]
    Transport(#[from] DaemonTransportError),
}
