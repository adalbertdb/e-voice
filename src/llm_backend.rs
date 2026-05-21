//! LLM backend trait and concrete adapters (Ollama, LM Studio).
//!
//! `LlmBackend` is the abstraction that `TextProcessor` talks to. Each adapter
//! handles the wire-format details of its own HTTP API.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

const BACKEND_TIMEOUT_SECS: u64 = 10;

// ─── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("http client error: {0}")]
    HttpClient(#[from] reqwest::Error),
    #[error("empty response from backend")]
    EmptyResponse,
}

impl BackendError {
    pub fn kind(&self) -> &'static str {
        match self {
            BackendError::HttpClient(err) if err.is_timeout() => "backend_timeout",
            BackendError::HttpClient(err) if err.is_status() => "backend_http_error",
            BackendError::HttpClient(_) => "backend_transport_error",
            BackendError::EmptyResponse => "empty_model_response",
        }
    }
}

// ─── Trait ────────────────────────────────────────────────────────────────────

/// Abstraction over an LLM backend server (Ollama, LM Studio, …).
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Warm-load the given model (no-op if the backend auto-loads).
    async fn load_model(&self, model: &str) -> Result<(), BackendError>;

    /// Send `prompt` to the backend and return the generated text.
    ///
    /// `model` selects the model to use (overrides the backend's default).
    /// `keep_alive_secs` controls how long the model stays resident after the
    /// request; `-1` means keep loaded indefinitely.
    async fn process(
        &self,
        model: &str,
        prompt: &str,
        keep_alive_secs: i64,
    ) -> Result<String, BackendError>;

    /// Ask the backend to evict the currently loaded model from memory.
    async fn unload_model(&self, model: &str) -> Result<(), BackendError>;

    /// Return the list of model names available on the backend.
    async fn list_models(&self) -> Result<Vec<String>, BackendError>;

    /// Return `true` when the backend server is reachable and responds with
    /// HTTP 200 within the timeout; `false` otherwise.
    async fn health_check(&self) -> bool;
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn build_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(Duration::from_secs(BACKEND_TIMEOUT_SECS))
        .build()
}

// ─── Ollama adapter ───────────────────────────────────────────────────────────

/// Converts `keep_alive_secs` to Ollama's native `keep_alive` string.
///
/// - `-1`  → `"-1"` (keep loaded indefinitely)
/// - `n≥0` → `"<n>s"`
fn ollama_keep_alive(secs: i64) -> String {
    if secs == -1 {
        "-1".to_owned()
    } else {
        format!("{secs}s")
    }
}

#[derive(Debug, Serialize)]
struct OllamaGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    keep_alive: String,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

#[derive(Debug, Serialize)]
struct OllamaUnloadRequest<'a> {
    model: &'a str,
    keep_alive: &'static str,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaTagEntry>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagEntry {
    name: String,
}

/// Ollama backend adapter.
#[derive(Debug, Clone)]
pub struct OllamaBackend {
    client: Client,
    url: String,
}

impl OllamaBackend {
    pub fn new(url: impl Into<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: build_client()?,
            url: url.into(),
        })
    }

    fn base_url(&self) -> &str {
        self.url.trim_end_matches('/')
    }
}

#[async_trait]
impl LlmBackend for OllamaBackend {
    async fn load_model(&self, _model: &str) -> Result<(), BackendError> {
        // Ollama loads models on demand — no explicit load endpoint needed.
        Ok(())
    }

    async fn process(
        &self,
        model: &str,
        prompt: &str,
        keep_alive_secs: i64,
    ) -> Result<String, BackendError> {
        let endpoint = format!("{}/api/generate", self.base_url());
        let payload = OllamaGenerateRequest {
            model,
            prompt,
            stream: false,
            keep_alive: ollama_keep_alive(keep_alive_secs),
        };

        debug!(model, endpoint, "sending request to ollama");

        let response = self
            .client
            .post(&endpoint)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        let body: OllamaGenerateResponse = response.json().await?;
        let trimmed = body.response.trim().to_owned();
        if trimmed.is_empty() {
            return Err(BackendError::EmptyResponse);
        }

        debug!(output_len = trimmed.len(), "ollama request completed");
        Ok(trimmed)
    }

    async fn unload_model(&self, model: &str) -> Result<(), BackendError> {
        // Ollama unloads a model by sending a generate request with keep_alive: "0".
        let endpoint = format!("{}/api/generate", self.base_url());
        let payload = OllamaUnloadRequest {
            model,
            keep_alive: "0",
        };

        self.client
            .post(&endpoint)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        debug!(model, "ollama model unloaded");
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<String>, BackendError> {
        let url = format!("{}/api/tags", self.base_url());
        let response = self.client.get(&url).send().await?.error_for_status()?;
        let body: OllamaTagsResponse = response.json().await?;
        Ok(body.models.into_iter().map(|m| m.name).collect())
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url());
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(err) => {
                warn!(%err, "ollama health check failed");
                false
            }
        }
    }
}

// ─── LM Studio adapter ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct LmStudioCompletionRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    /// How many seconds to keep the model resident after the request.
    ttl: i64,
}

#[derive(Debug, Deserialize)]
struct LmStudioCompletionResponse {
    choices: Vec<LmStudioChoice>,
}

#[derive(Debug, Deserialize)]
struct LmStudioChoice {
    text: String,
}

#[derive(Debug, Deserialize)]
struct LmStudioModelsResponse {
    data: Vec<LmStudioModelEntry>,
}

#[derive(Debug, Deserialize)]
struct LmStudioModelEntry {
    id: String,
}

/// LM Studio backend adapter.
#[derive(Debug, Clone)]
pub struct LmStudioBackend {
    client: Client,
    url: String,
}

impl LmStudioBackend {
    pub fn new(url: impl Into<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: build_client()?,
            url: url.into(),
        })
    }

    fn base_url(&self) -> &str {
        self.url.trim_end_matches('/')
    }
}

#[async_trait]
impl LlmBackend for LmStudioBackend {
    async fn load_model(&self, model: &str) -> Result<(), BackendError> {
        // LM Studio loads models lazily; the first completion request will load it.
        // Optionally call /v1/models/load if an eager load is needed.
        let endpoint = format!("{}/v1/models/load", self.base_url());
        let payload = serde_json::json!({ "identifier": model });
        self.client
            .post(&endpoint)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn process(
        &self,
        model: &str,
        prompt: &str,
        keep_alive_secs: i64,
    ) -> Result<String, BackendError> {
        let endpoint = format!("{}/v1/completions", self.base_url());
        let payload = LmStudioCompletionRequest {
            model,
            prompt,
            stream: false,
            ttl: keep_alive_secs,
        };

        debug!(model, endpoint, "sending request to lm studio");

        let response = self
            .client
            .post(&endpoint)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        let body: LmStudioCompletionResponse = response.json().await?;
        let trimmed = body
            .choices
            .into_iter()
            .next()
            .map(|c| c.text.trim().to_owned())
            .unwrap_or_default();

        if trimmed.is_empty() {
            return Err(BackendError::EmptyResponse);
        }

        debug!(output_len = trimmed.len(), "lm studio request completed");
        Ok(trimmed)
    }

    async fn unload_model(&self, model: &str) -> Result<(), BackendError> {
        // LM Studio unloads via DELETE /v1/models/{model_identifier}.
        let endpoint = format!("{}/v1/models/{}", self.base_url(), model);
        self.client
            .delete(&endpoint)
            .send()
            .await?
            .error_for_status()?;
        debug!(model, "lm studio model unloaded");
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<String>, BackendError> {
        let url = format!("{}/v1/models", self.base_url());
        let response = self.client.get(&url).send().await?.error_for_status()?;
        let body: LmStudioModelsResponse = response.json().await?;
        Ok(body.data.into_iter().map(|m| m.id).collect())
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/v1/models", self.base_url());
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(err) => {
                warn!(%err, "lm studio health check failed");
                false
            }
        }
    }
}

// ─── Factory ──────────────────────────────────────────────────────────────────

/// Construct the appropriate [`LlmBackend`] from the application config.
pub fn from_config(
    config: &crate::config::AppConfig,
) -> Result<Box<dyn LlmBackend>, reqwest::Error> {
    match config.llm.backend {
        crate::config::Backend::Ollama => {
            Ok(Box::new(OllamaBackend::new(config.llm.url.clone())?))
        }
        crate::config::Backend::LmStudio => {
            Ok(Box::new(LmStudioBackend::new(config.llm.url.clone())?))
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ── Mock server helpers ───────────────────────────────────────────────────

    /// Spawn a minimal HTTP/1.1 server on an ephemeral port.
    /// Returns `(url, received_body_rx)` where `received_body_rx` yields the
    /// raw request body for each accepted connection.
    async fn spawn_mock_server(
        response_body: &'static str,
        status: u16,
    ) -> (String, tokio::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("should have local addr");
        let (tx, rx) = tokio::sync::mpsc::channel(8);

        tokio::spawn(async move {
            for _ in 0..10 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let raw = String::from_utf8_lossy(&buf[..n]).to_string();

                    // Extract body after blank line separator.
                    let body = raw
                        .split_once("\r\n\r\n")
                        .map(|(_, b)| b.to_owned())
                        .unwrap_or_default();
                    let _ = tx.send(body).await;

                    let http_response = format!(
                        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    );
                    let _ = stream.write_all(http_response.as_bytes()).await;
                });
            }
        });

        (format!("http://{addr}"), rx)
    }

    /// Spawn a mock server that does not respond (simulates timeout / unreachable).
    async fn spawn_silent_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("should have local addr");

        tokio::spawn(async move {
            // Accept but never write a response.
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf).await;
                // Hold the connection open indefinitely.
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(stream);
            }
        });

        format!("http://{addr}")
    }

    // ── OllamaBackend tests ───────────────────────────────────────────────────

    #[test]
    fn test_ollama_backend_keep_alive_mapping() {
        assert_eq!(ollama_keep_alive(60), "60s");
        assert_eq!(ollama_keep_alive(0), "0s");
        assert_eq!(ollama_keep_alive(-1), "-1");
    }

    #[tokio::test]
    async fn test_ollama_backend_process_sends_correct_payload() {
        let response_json =
            r#"{"model":"llama3.2:1b","response":"cleaned text","done":true}"#;
        let (url, mut body_rx) = spawn_mock_server(response_json, 200).await;

        let backend = OllamaBackend::new(&url).expect("backend should build");
        let result = backend
            .process("llama3.2:1b", "test prompt", 60)
            .await
            .expect("process should succeed");

        assert_eq!(result, "cleaned text");

        let body = body_rx
            .recv()
            .await
            .expect("mock server should have received a request");
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("body should be valid JSON");

        assert_eq!(parsed["model"], "llama3.2:1b");
        assert_eq!(parsed["prompt"], "test prompt");
        assert_eq!(parsed["stream"], false);
        assert_eq!(parsed["keep_alive"], "60s");
    }

    #[tokio::test]
    async fn test_ollama_backend_list_models_parses_response() {
        let response_json = r#"{"models":[{"name":"llama3.2:1b"},{"name":"mistral:7b"}]}"#;
        let (url, _rx) = spawn_mock_server(response_json, 200).await;

        let backend = OllamaBackend::new(&url).expect("backend should build");
        let models = backend.list_models().await.expect("list_models should succeed");

        assert_eq!(models, vec!["llama3.2:1b", "mistral:7b"]);
    }

    #[tokio::test]
    async fn test_ollama_backend_unload_hits_api_generate() {
        // Ollama unloads by posting to /api/generate with keep_alive: "0".
        let response_json = r#"{"model":"llama3.2:1b","response":"","done":true}"#;
        let (url, mut body_rx) = spawn_mock_server(response_json, 200).await;

        let backend = OllamaBackend::new(&url).expect("backend should build");
        backend
            .unload_model("llama3.2:1b")
            .await
            .expect("unload_model should succeed");

        let body = body_rx
            .recv()
            .await
            .expect("mock server should have received a request");
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("body should be valid JSON");

        assert_eq!(parsed["model"], "llama3.2:1b");
        assert_eq!(parsed["keep_alive"], "0");
    }

    #[tokio::test]
    async fn test_ollama_health_check_returns_true_on_200() {
        let (url, _rx) = spawn_mock_server(r#"{"models":[]}"#, 200).await;
        let backend = OllamaBackend::new(&url).expect("backend should build");
        assert!(backend.health_check().await);
    }

    #[tokio::test]
    async fn test_ollama_health_check_returns_false_on_timeout() {
        // Use a port that is not listening — connection refused is fast.
        let backend = OllamaBackend::new("http://127.0.0.1:1").expect("backend should build");
        assert!(!backend.health_check().await);
    }

    // ── LmStudioBackend tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_lm_studio_backend_process_sends_correct_payload() {
        let response_json =
            r#"{"choices":[{"text":"lm studio output","finish_reason":"stop"}]}"#;
        let (url, mut body_rx) = spawn_mock_server(response_json, 200).await;

        let backend = LmStudioBackend::new(&url).expect("backend should build");
        let result = backend
            .process("my-model", "test prompt", 60)
            .await
            .expect("process should succeed");

        assert_eq!(result, "lm studio output");

        let body = body_rx
            .recv()
            .await
            .expect("mock server should have received a request");
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("body should be valid JSON");

        assert_eq!(parsed["model"], "my-model");
        assert_eq!(parsed["prompt"], "test prompt");
        assert_eq!(parsed["stream"], false);
        assert_eq!(parsed["ttl"], 60);
    }

    #[tokio::test]
    async fn test_lm_studio_backend_unload_calls_delete_endpoint() {
        // LM Studio unloads via DELETE /v1/models/{model_id}.
        // Spawn a server that accepts any request and returns 200.
        let (url, mut body_rx) = spawn_mock_server(r#"{}"#, 200).await;

        let backend = LmStudioBackend::new(&url).expect("backend should build");
        backend
            .unload_model("my-model")
            .await
            .expect("unload_model should succeed");

        // The body for a DELETE request is empty; just verify the server received it.
        let _ = body_rx.recv().await;
    }

    #[tokio::test]
    async fn test_lm_studio_health_check_returns_true_on_200() {
        let (url, _rx) = spawn_mock_server(r#"{"object":"list","data":[]}"#, 200).await;
        let backend = LmStudioBackend::new(&url).expect("backend should build");
        assert!(backend.health_check().await);
    }

    #[tokio::test]
    async fn test_lm_studio_health_check_returns_false_on_timeout() {
        let backend =
            LmStudioBackend::new("http://127.0.0.1:1").expect("backend should build");
        assert!(!backend.health_check().await);
    }

    // ── keep_alive_secs mapping ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_keep_alive_secs_60_maps_to_ollama_60s() {
        let response_json =
            r#"{"model":"llama3.2:1b","response":"ok","done":true}"#;
        let (url, mut body_rx) = spawn_mock_server(response_json, 200).await;

        let backend = OllamaBackend::new(&url).expect("backend should build");
        let _ = backend.process("llama3.2:1b", "prompt", 60).await;

        let body = body_rx.recv().await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["keep_alive"], "60s");
    }

    #[tokio::test]
    async fn test_keep_alive_secs_60_maps_to_lm_studio_ttl_60() {
        let response_json = r#"{"choices":[{"text":"ok","finish_reason":"stop"}]}"#;
        let (url, mut body_rx) = spawn_mock_server(response_json, 200).await;

        let backend = LmStudioBackend::new(&url).expect("backend should build");
        let _ = backend.process("my-model", "prompt", 60).await;

        let body = body_rx.recv().await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["ttl"], 60);
    }

    #[tokio::test]
    async fn test_health_check_returns_false_on_silent_server() {
        // The silent server accepts the TCP connection but never sends a response,
        // so the HTTP client will time out.
        let url = spawn_silent_server().await;

        // Override to a shorter timeout for the test.
        let client = Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();

        // Test Ollama backend with custom short-timeout client.
        let backend = OllamaBackend { client: client.clone(), url: url.clone() };
        assert!(!backend.health_check().await);

        // Test LM Studio backend with custom short-timeout client.
        let backend = LmStudioBackend { client, url };
        assert!(!backend.health_check().await);
    }
}
