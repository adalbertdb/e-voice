//! Unix socket daemon for processing requests and managing runtime state.

use crate::config::{AppConfig, ConfigError, load_state, save_state};
use crate::modes::Mode;
use crate::processor::TextProcessor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub struct AppState {
    pub mode: Mode,
    pub processor: TextProcessor,
}

impl AppState {
    pub fn mode(&self) -> Mode {
        self.mode.clone()
    }

    pub fn set_mode(&mut self, mode: Mode) -> Result<(), ConfigError> {
        self.mode = mode.clone();
        save_state(&mode)
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Process {
        text: String,
        request_id: Option<String>,
    },
    SetMode { mode: String },
    GetStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Response {
    Text(String),
    ModeChanged { mode: String },
    Status { mode: String, version: String },
    Error(String),
}

pub struct Daemon {
    socket_path: PathBuf,
    state: SharedState,
}

impl Daemon {
    pub fn new(config: AppConfig) -> Result<Self, DaemonError> {
        Self::new_with_socket_path(config, socket_path()?)
    }

    pub fn new_with_socket_path(
        config: AppConfig,
        socket_path: PathBuf,
    ) -> Result<Self, DaemonError> {
        let processor = TextProcessor::new(config.clone())?;
        let default_mode = config.default_mode();
        let mode = load_state(&default_mode)?;

        Ok(Self {
            socket_path,
            state: Arc::new(Mutex::new(AppState { mode, processor })),
        })
    }

    pub fn shared_state(&self) -> SharedState {
        Arc::clone(&self.state)
    }

    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), DaemonError> {
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        info!(socket = %self.socket_path.display(), "daemon listening on unix socket");

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    let (stream, _) = accept_result?;
                    let state = Arc::clone(&self.state);
                    tokio::spawn(async move {
                        if let Err(err) = handle_connection(stream, state).await {
                            warn!(error = %err, "daemon connection handler failed");
                        }
                    });
                }
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        break;
                    }
                }
            }
        }

        self.cleanup_socket();
        Ok(())
    }

    pub fn cleanup_socket(&self) {
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
            info!(socket = %self.socket_path.display(), "daemon socket removed");
        }
    }
}

pub async fn handle_connection(stream: UnixStream, state: SharedState) -> Result<(), DaemonError> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let request: Result<Request, _> = serde_json::from_str(&line);
        let response = match request {
            Ok(req) => handle_request(req, Arc::clone(&state)).await,
            Err(err) => Response::Error(format!("invalid request: {err}")),
        };

        let payload = serde_json::to_string(&response)?;
        writer.write_all(payload.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

pub async fn handle_request(request: Request, state: SharedState) -> Response {
    match request {
        Request::Process { text, request_id } => {
            let request_id = request_id.unwrap_or_else(|| "unknown-request".to_owned());
            info!(request_id = %request_id, input_len = text.len(), "received process request");
            let (mode, processor) = {
                let guard = match state.lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        error!(request_id = %request_id, "failed to lock app state for processing");
                        return Response::Error("failed to lock app state".to_owned());
                    }
                };
                (guard.mode(), guard.processor.clone())
            };

            debug!(request_id = %request_id, mode = %mode, "forwarding text to processor");
            let processed = processor.process(&mode, &text, &request_id).await;
            info!(request_id = %request_id, output_len = processed.len(), "process request completed");
            Response::Text(processed)
        }
        Request::SetMode { mode } => match mode.parse::<Mode>() {
            Ok(parsed_mode) => {
                let set_result = match state.lock() {
                    Ok(mut guard) => guard.set_mode(parsed_mode.clone()),
                    Err(_) => {
                        error!("failed to lock app state for mode change");
                        return Response::Error("failed to lock app state".to_owned());
                    }
                };

                if let Err(err) = set_result {
                    error!(error = %err, "failed to persist mode");
                    return Response::Error(format!("failed to persist mode: {err}"));
                }

                info!(mode = %parsed_mode, "mode changed");
                Response::ModeChanged {
                    mode: parsed_mode.to_string(),
                }
            }
            Err(err) => Response::Error(err.to_string()),
        },
        Request::GetStatus => {
            let mode = {
                match state.lock() {
                    Ok(guard) => guard.mode().to_string(),
                    Err(_) => return Response::Error("failed to lock app state".to_owned()),
                }
            };

            Response::Status {
                mode,
                version: VERSION.to_owned(),
            }
        }
    }
}

pub fn socket_path() -> Result<PathBuf, DaemonError> {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR")
        && !runtime_dir.is_empty()
    {
        return Ok(PathBuf::from(runtime_dir).join("e-voice.sock"));
    }

    let uid = std::env::var("UID").unwrap_or_else(|_| "unknown".to_owned());
    Ok(PathBuf::from(format!("/tmp/e-voice-{uid}.sock")))
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("processor error: {0}")]
    Processor(#[from] crate::processor::ProcessorError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::Response;

    #[test]
    fn serializes_text_response_with_tagged_payload() {
        let json = serde_json::to_string(&Response::Text("hello".to_owned()))
            .expect("text response should serialize");

        assert_eq!(json, r#"{"type":"text","payload":"hello"}"#);
    }
}
