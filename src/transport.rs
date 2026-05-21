//! Unified Unix socket transport used by both CLI commands and the system tray.

use crate::daemon::{Request, Response};
use std::path::PathBuf;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Low-level transport that speaks the daemon's newline-delimited JSON protocol.
///
/// The caller is responsible for resolving the socket path before constructing
/// this type — no path resolution happens internally.
#[derive(Debug, Clone)]
pub struct DaemonTransport {
    socket_path: PathBuf,
}

impl DaemonTransport {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub async fn send(&self, req: Request) -> Result<Response, DaemonTransportError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        let payload = serde_json::to_string(&req)?;
        writer.write_all(payload.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        let line = lines
            .next_line()
            .await?
            .ok_or_else(|| DaemonTransportError::Protocol("daemon closed connection".to_owned()))?;

        let response = serde_json::from_str::<Response>(&line)?;
        Ok(response)
    }
}

#[derive(Debug, Error)]
pub enum DaemonTransportError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::daemon::{Daemon, Request, Response};
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::watch;
    use tokio::time::{Duration, sleep};

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn daemon_transport_get_status_and_set_model_roundtrip() {
        let _guard = test_lock().lock().expect("test mutex poisoned");

        let temp = tempfile::tempdir().expect("failed to create temp dir");
        // SAFETY: tests are serialized via global mutex, so process-wide env mutation is controlled.
        unsafe {
            std::env::set_var("HOME", temp.path());
        }

        let socket_path = temp.path().join("e-voice-test.sock");
        let daemon = Daemon::new_with_socket_path(AppConfig::default(), socket_path.clone())
            .expect("failed to create daemon");

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let daemon_task = tokio::spawn(async move { daemon.run(shutdown_rx).await });

        for _ in 0..20 {
            if socket_path.exists() {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        assert!(socket_path.exists(), "socket file was not created");

        let transport = DaemonTransport::new(socket_path.clone());

        let status = transport
            .send(Request::GetStatus)
            .await
            .expect("GetStatus should succeed");
        match status {
            Response::Status {
                model,
                version,
                profile: _,
            } => {
                assert!(!model.is_empty(), "model should not be empty");
                assert!(!version.is_empty(), "version should not be empty");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let changed = transport
            .send(Request::SetModel {
                model: "llama3.2:3b".to_owned(),
            })
            .await
            .expect("SetModel should succeed");
        match changed {
            Response::ModelChanged { model } => assert_eq!(model, "llama3.2:3b"),
            other => panic!("unexpected response: {other:?}"),
        }

        let status_after = transport
            .send(Request::GetStatus)
            .await
            .expect("GetStatus after SetModel should succeed");
        match status_after {
            Response::Status {
                model,
                version,
                profile: _,
            } => {
                assert_eq!(model, "llama3.2:3b");
                assert!(!version.is_empty(), "version should not be empty");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let _ = shutdown_tx.send(true);
        let daemon_result = daemon_task.await.expect("daemon task join should succeed");
        assert!(daemon_result.is_ok(), "daemon should shutdown cleanly");
    }
}
