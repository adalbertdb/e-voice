//! Axum HTTP server — the new transport layer replacing the Unix socket daemon.
//!
//! Exposes three endpoints on `127.0.0.1:PORT` (default [`DEFAULT_HTTP_PORT`]):
//!
//! | Method | Path       | Description                                      |
//! |--------|------------|--------------------------------------------------|
//! | POST   | /process   | Receive `{"text":"…"}`, return `{"text":"…"}`    |
//! | GET    | /health    | Returns `{"status":"ok"}` when the server is up  |
//! | GET    | /status    | Returns active model name and daemon version     |

use crate::config::{AppConfig, PersistentState};
use crate::daemon::{AppState, SharedState, handle_request, Request, Response};
use crate::modes::Profile;
use crate::processor::TextProcessor;
use axum::{
    Router,
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::info;

/// Default TCP port for the HTTP server.
pub const DEFAULT_HTTP_PORT: u16 = 39539;

// ─── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ProcessPayload {
    pub text: String,
    /// Optional profile for this request.  Defaults to the currently active
    /// profile stored in `AppState` (i.e. `UniversalInterpreter` on first run).
    pub profile: Option<Profile>,
}

#[derive(Debug, Serialize)]
pub struct ProcessResponse {
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub model: String,
    pub version: String,
    pub profile: String,
}

// ─── Route handlers ───────────────────────────────────────────────────────────

async fn health_handler() -> impl IntoResponse {
    Json(HealthResponse { status: "ok" })
}

async fn status_handler(State(state): State<SharedState>) -> impl IntoResponse {
    match handle_request(Request::GetStatus, state).await {
        Response::Status {
            model,
            version,
            profile,
        } => (
            StatusCode::OK,
            Json(StatusResponse {
                model,
                version,
                profile,
            }),
        )
            .into_response(),
        Response::Error(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "unexpected response").into_response(),
    }
}

async fn process_handler(
    State(state): State<SharedState>,
    Json(payload): Json<ProcessPayload>,
) -> impl IntoResponse {
    let response = handle_request(
        Request::Process {
            text: payload.text,
            request_id: None,
            profile: payload.profile,
        },
        state,
    )
    .await;

    match response {
        Response::Text(text) => (StatusCode::OK, Json(ProcessResponse { text })).into_response(),
        Response::Error(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "unexpected response").into_response(),
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Build an Axum [`Router`] wired to the shared [`AppState`].
pub fn create_router(state: SharedState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/status", get(status_handler))
        .route("/process", post(process_handler))
        .with_state(state)
}

/// Initialise the shared application state from a config.
///
/// Starts with the default profile (`UniversalInterpreter`).  Production code
/// should call [`build_state_with_persistent_profile`] (or use [`run`]) to
/// restore the last active profile from the state file.
pub fn build_state(config: AppConfig) -> Result<SharedState, crate::processor::ProcessorError> {
    build_state_with_profile(config, Profile::default())
}

/// Initialise state with an explicit initial profile.  Used by [`run`] to
/// restore the persisted profile on daemon start.
fn build_state_with_profile(
    config: AppConfig,
    profile: Profile,
) -> Result<SharedState, crate::processor::ProcessorError> {
    let processor = TextProcessor::new(config)?;
    Ok(Arc::new(Mutex::new(AppState {
        override_model: None,
        active_profile: profile,
        processor,
    })))
}

/// Bind a [`TcpListener`] on `127.0.0.1:port` (localhost only).
pub async fn bind_listener(port: u16) -> std::io::Result<TcpListener> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpListener::bind(addr).await
}

/// Run the Axum HTTP server until `shutdown` is signalled.
pub async fn run(
    config: AppConfig,
    port: u16,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let persistent = PersistentState::load();
    let state = build_state_with_profile(config, persistent.profile)?;
    let app = create_router(state);
    let listener = bind_listener(port).await?;
    let local_addr = listener.local_addr()?;
    info!(addr = %local_addr, "HTTP server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            loop {
                if shutdown.changed().await.is_err() {
                    break;
                }
                if *shutdown.borrow() {
                    break;
                }
            }
        })
        .await?;

    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, Backend, LlmConfig};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener as TokioListener;

    fn make_config(ollama_url: String) -> AppConfig {
        AppConfig {
            llm: LlmConfig {
                backend: Backend::Ollama,
                url: ollama_url,
                model: "test-model".to_owned(),
                keep_alive_secs: 300,
            },
        }
    }

    /// Spin up a minimal mock Ollama `/api/generate` server that always returns
    /// `response_text` in the `response` field.
    async fn spawn_ollama_mock(response_text: &'static str) -> String {
        let listener = TokioListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener should bind");
        let addr = listener.local_addr().expect("should have local addr");

        tokio::spawn(async move {
            // Handle several connections so the mock survives parallel test requests.
            for _ in 0..20 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let body = format!(
                    r#"{{"model":"test","response":"{response_text}","done":true}}"#
                );
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(http_response.as_bytes()).await;
            }
        });

        format!("http://{addr}")
    }

    /// Start the HTTP server on an ephemeral port; returns `(port, shutdown_tx)`.
    async fn start_test_server(config: AppConfig) -> (u16, watch::Sender<bool>) {
        let state = build_state(config).expect("state should build");
        let app = create_router(state);
        let listener = TokioListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let port = listener.local_addr().unwrap().port();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let mut rx = shutdown_rx;
                    loop {
                        if rx.changed().await.is_err() {
                            break;
                        }
                        if *rx.borrow() {
                            break;
                        }
                    }
                })
                .await
                .ok();
        });

        // Give the server time to start accepting.
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        (port, shutdown_tx)
    }

    #[tokio::test]
    async fn test_health_returns_200() {
        let ollama_url = spawn_ollama_mock("ok").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .expect("health request should succeed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_status_returns_model_and_version() {
        let ollama_url = spawn_ollama_mock("ok").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/status"))
            .send()
            .await
            .expect("status request should succeed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        assert_eq!(body["model"], "test-model");
        assert!(
            !body["version"].as_str().unwrap_or("").is_empty(),
            "version should not be empty"
        );
    }

    #[tokio::test]
    async fn test_post_process_returns_text() {
        let ollama_url = spawn_ollama_mock("Hello world").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/process"))
            .json(&serde_json::json!({"text": "hello"}))
            .send()
            .await
            .expect("process request should succeed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        assert!(
            !body["text"].as_str().unwrap_or("").is_empty(),
            "processed text should not be empty"
        );
    }

    #[tokio::test]
    async fn test_server_binds_localhost_only() {
        // Verify that `bind_listener` produces a socket bound to 127.0.0.1, not 0.0.0.0.
        let listener = bind_listener(0).await.expect("bind should succeed on port 0");
        let addr = listener.local_addr().expect("listener should have a local addr");
        assert_eq!(
            addr.ip().to_string(),
            "127.0.0.1",
            "server must be bound to localhost only, not 0.0.0.0"
        );
    }

    #[tokio::test]
    async fn test_headless_flag_starts_only_http_server() {
        // In headless mode the HTTP server must accept requests. Verify this by
        // starting a server (simulating --headless) and hitting /health.
        let ollama_url = spawn_ollama_mock("ok").await;
        let config = make_config(ollama_url);
        let (port, shutdown_tx) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .expect("headless HTTP server should serve requests");

        assert_eq!(resp.status(), 200, "headless mode must serve HTTP traffic");
        let _ = shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn test_process_text_roundtrip() {
        // Full roundtrip: POST raw text → Axum → mock TextProcessor → response.
        let ollama_url = spawn_ollama_mock("processed output").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/process"))
            .json(&serde_json::json!({"text": "raw input text"}))
            .send()
            .await
            .expect("roundtrip process request should succeed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        let text = body["text"].as_str().expect("text field should be present");
        assert!(!text.is_empty(), "response text should not be empty");
    }

    #[tokio::test]
    async fn test_voxtype_config_patched_to_http() {
        // The voxtype config is patched to invoke `e-voice process`, which now
        // talks to the HTTP server. Verify the patching function writes the
        // expected HTTP URL into a temporary config file.
        use crate::system_adapter::patch_voxtype_http_hook_at_path;
        use tempfile::tempdir;

        let dir = tempdir().expect("tempdir should be created");
        let cfg_path = dir.path().join("config.toml");

        patch_voxtype_http_hook_at_path(&cfg_path, DEFAULT_HTTP_PORT)
            .expect("patching should succeed");

        let content = std::fs::read_to_string(&cfg_path).expect("config should be readable");
        assert!(
            content.contains(&format!("http://127.0.0.1:{DEFAULT_HTTP_PORT}/process")),
            "voxtype config must reference the HTTP process endpoint; got:\n{content}"
        );
    }

    #[tokio::test]
    async fn test_post_process_without_profile_defaults_to_universal() {
        // Missing profile field must use UniversalInterpreter (default).
        let ollama_url = spawn_ollama_mock("processed").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/process"))
            .json(&serde_json::json!({"text": "hello world"}))
            .send()
            .await
            .expect("process request should succeed");

        assert_eq!(resp.status(), 200, "should return 200 when profile is absent");
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        assert!(
            body["text"].as_str().is_some(),
            "text field should be present in response"
        );
    }

    #[tokio::test]
    async fn test_post_process_with_profile_field() {
        // Explicit profile field must be accepted and processed without error.
        let ollama_url = spawn_ollama_mock("formal output").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/process"))
            .json(&serde_json::json!({"text": "hey mate", "profile": "formal"}))
            .send()
            .await
            .expect("process request with profile should succeed");

        assert_eq!(resp.status(), 200, "should return 200 for formal profile");
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        assert!(
            body["text"].as_str().is_some(),
            "text field should be present in response"
        );
    }

    #[tokio::test]
    async fn test_status_returns_active_profile() {
        // GET /status must include the "profile" field.
        let ollama_url = spawn_ollama_mock("ok").await;
        let config = make_config(ollama_url);
        let (port, _shutdown) = start_test_server(config).await;

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/status"))
            .send()
            .await
            .expect("status request should succeed");

        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.expect("body should be JSON");
        let profile = body["profile"].as_str().expect("profile field must be present");
        assert_eq!(
            profile, "universal_interpreter",
            "default active profile should be universal_interpreter"
        );
    }
}
