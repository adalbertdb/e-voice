//! Unix socket client helpers used by CLI commands.

use crate::daemon::{Request, Response, socket_path};
use std::path::PathBuf;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone)]
pub struct DaemonClient {
    socket_path: Option<PathBuf>,
}

impl DaemonClient {
    pub fn new() -> Self {
        Self { socket_path: None }
    }

    #[cfg(test)]
    pub fn with_socket_path(path: PathBuf) -> Self {
        Self {
            socket_path: Some(path),
        }
    }

    pub async fn send(&self, request: Request) -> Result<Response, ClientError> {
        let path = match &self.socket_path {
            Some(path) => path.clone(),
            None => socket_path()?,
        };
        let stream = UnixStream::connect(path).await?;
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        let payload = serde_json::to_string(&request)?;
        writer.write_all(payload.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        let line = lines
            .next_line()
            .await?
            .ok_or_else(|| ClientError::Protocol("daemon closed connection".to_owned()))?;

        let response = serde_json::from_str::<Response>(&line)?;
        Ok(response)
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("daemon error: {0}")]
    Daemon(#[from] crate::daemon::DaemonError),
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
    async fn daemon_client_get_status_and_set_mode_roundtrip() {
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

        let client = DaemonClient::with_socket_path(socket_path.clone());

        let status = client
            .send(Request::GetStatus)
            .await
            .expect("GetStatus should succeed");
        match status {
            Response::Status { mode, .. } => {
                assert!(!mode.is_empty(), "mode should not be empty");
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let changed = client
            .send(Request::SetMode {
                mode: "bullet".to_owned(),
            })
            .await
            .expect("SetMode should succeed");
        match changed {
            Response::ModeChanged { mode } => assert_eq!(mode, "bullet"),
            other => panic!("unexpected response: {other:?}"),
        }

        let status_after = client
            .send(Request::GetStatus)
            .await
            .expect("GetStatus after SetMode should succeed");
        match status_after {
            Response::Status { mode, .. } => assert_eq!(mode, "bullet"),
            other => panic!("unexpected response: {other:?}"),
        }

        let _ = shutdown_tx.send(true);
        let daemon_result = daemon_task.await.expect("daemon task join should succeed");
        assert!(daemon_result.is_ok(), "daemon should shutdown cleanly");
    }
}
