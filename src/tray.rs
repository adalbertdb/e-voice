//! System tray for STT language and LLM model switching via ksni (StatusNotifierItem DBus).

use crate::config::{AppConfig, config_file_path};
use crate::daemon::{Daemon, Request, Response, socket_path};
use crate::system_adapter::{STT_PRESETS, SystemAdapter};
use crate::transport::{DaemonTransport, DaemonTransportError};
use ksni::TrayMethods;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

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
            ksni::MenuItem::Separator,
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

// ─── Health helpers ───────────────────────────────────────────────────────────

async fn check_health<A: SystemAdapter>(adapter: &A, ollama_url: &str) -> HealthInfo {
    HealthInfo {
        ollama_reachable: adapter.check_ollama_health(ollama_url).await,
        voxtype_running: adapter.is_stt_service_running(),
    }
}

// ─── Command helpers ──────────────────────────────────────────────────────────

/// Apply an STT language change: write config then restart the service.
///
/// Returns the `(label, model, language)` triple on success so the tray state
/// can be updated, or `None` when the config write fails (in which case the
/// service is NOT restarted).
pub(crate) async fn apply_stt_language_cmd<A: SystemAdapter>(
    adapter: &A,
    model: &str,
    language: &str,
) -> Option<(String, String, String)> {
    match adapter.write_voxtype_stt_config(model, language) {
        Ok(()) => {
            info!(%model, %language, "voxtype STT config updated");
            if let Err(err) = adapter.restart_stt_service() {
                error!(%err, "failed to restart voxtype service");
            } else {
                info!("voxtype service restarted");
            }
            let label = STT_PRESETS
                .iter()
                .find(|(_, m, l)| *m == model && *l == language)
                .map(|(lbl, _, _)| lbl.to_string())
                .unwrap_or_else(|| language.to_string());
            Some((label, model.to_string(), language.to_string()))
        }
        Err(err) => {
            error!(%err, "failed to write voxtype config");
            None
        }
    }
}

// ─── Public entry-point ───────────────────────────────────────────────────────

pub async fn run<A: SystemAdapter + Send + Sync>(adapter: &A) -> Result<(), TrayError> {
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

    let (stt_label, stt_model, stt_language) = adapter
        .read_voxtype_stt_config()
        .unwrap_or_else(|_| ("Unknown".into(), "base.en".into(), "en".into()));
    let initial_health = check_health(adapter, &ollama_url).await;
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
                        if let Some((label, model, lang)) =
                            apply_stt_language_cmd(adapter, &model, &language).await
                        {
                            let notification_label = label.clone();
                            let notification_model = model.clone();
                            tray_handle.update(|tray: &mut TrayState| {
                                tray.stt_label = label;
                                tray.stt_model = model;
                                tray.stt_language = lang;
                            }).await;
                            let _ = adapter.send_notification(
                                "STT Language",
                                &format!("Switched to {} ({})", notification_label, notification_model),
                            );
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
                                        let _ = adapter.send_notification(
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
                            let _ = adapter.open_file(&path);
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
                let health = check_health(adapter, &ollama_url).await;
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

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("config error: {0}")]
    Config(String),
    #[error("daemon error: {0}")]
    Daemon(String),
    #[error("tray spawn error: {0}")]
    TraySpawn(String),
    #[error("adapter error: {0}")]
    Adapter(String),
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_adapter::fake::FakeSystemAdapter;

    // ── Health ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_tick_reflects_ollama_unreachable() {
        let adapter = FakeSystemAdapter::new(true, false);
        let health = check_health(&adapter, "http://localhost:11434").await;
        assert!(!health.ollama_reachable, "ollama should be unreachable");
        assert!(health.voxtype_running, "voxtype should be running");
    }

    #[tokio::test]
    async fn health_tick_reflects_ollama_recovered() {
        let adapter = FakeSystemAdapter::new(true, true);
        let health = check_health(&adapter, "http://localhost:11434").await;
        assert!(health.ollama_reachable, "ollama should be reachable");
    }

    // ── SetSttLanguage ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_stt_language_calls_write_then_restart() {
        let adapter = FakeSystemAdapter::new(true, true);
        let result = apply_stt_language_cmd(&adapter, "base", "es").await;
        assert!(result.is_some());
        let calls = adapter.calls();
        let write_pos = calls
            .iter()
            .position(|c| c.starts_with("write_voxtype_stt_config"))
            .expect("write_voxtype_stt_config not called");
        let restart_pos = calls
            .iter()
            .position(|c| c == "restart_stt_service")
            .expect("restart_stt_service not called");
        assert!(
            write_pos < restart_pos,
            "write must be called before restart"
        );
    }

    #[tokio::test]
    async fn set_stt_language_no_restart_when_write_fails() {
        let mut adapter = FakeSystemAdapter::new(true, true);
        adapter.write_stt_fails = true;
        let result = apply_stt_language_cmd(&adapter, "base", "es").await;
        assert!(result.is_none(), "should return None on write failure");
        assert!(
            !adapter.calls().iter().any(|c| c == "restart_stt_service"),
            "restart_stt_service must NOT be called when write fails"
        );
    }
}
