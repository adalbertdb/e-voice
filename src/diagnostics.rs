use crate::config::AppConfig;
use crate::daemon::{Request, Response};
use crate::system_adapter::SystemAdapter;
use crate::transport::DaemonTransport;
use reqwest::StatusCode;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Fail,
    Info,
    #[allow(dead_code)]
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub status: CheckStatus,
    pub message: String,
}

impl CheckResult {
    fn new(status: CheckStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelTag>,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaModelTag {
    name: String,
}

pub async fn run<A: SystemAdapter>(
    config: &AppConfig,
    transport: &DaemonTransport,
    socket_path: PathBuf,
    system: &A,
) -> Vec<CheckResult> {
    let configured_models = configured_models(config);
    let mut results = vec![
        CheckResult::new(
            CheckStatus::Info,
            format!("llm url: {}", config.llm.url),
        ),
        CheckResult::new(
            CheckStatus::Info,
            format!("configured models: {}", configured_models.join(", ")),
        ),
    ];

    if socket_path.exists() {
        results.push(CheckResult::new(
            CheckStatus::Ok,
            format!("daemon socket exists: {}", socket_path.display()),
        ));
    } else {
        results.push(CheckResult::new(
            CheckStatus::Fail,
            format!("daemon socket missing: {}", socket_path.display()),
        ));
        return results;
    }

    match transport.send(Request::GetStatus).await {
        Ok(Response::Status { version, .. }) => {
            results.push(CheckResult::new(
                CheckStatus::Ok,
                format!("daemon reachable: version={version}"),
            ));
        }
        Ok(Response::Error(err)) => {
            results.push(CheckResult::new(
                CheckStatus::Fail,
                format!("daemon error response: {err}"),
            ));
        }
        Ok(other) => {
            results.push(CheckResult::new(
                CheckStatus::Fail,
                format!("unexpected daemon response: {other:?}"),
            ));
        }
        Err(err) => {
            results.push(CheckResult::new(
                CheckStatus::Fail,
                format!("daemon unreachable: {err}"),
            ));
        }
    }

    if !system.check_ollama_health(&config.llm.url).await {
        results.push(CheckResult::new(
            CheckStatus::Fail,
            "ollama unreachable: health check failed",
        ));
        return results;
    }

    let tags_url = format!("{}/api/tags", config.llm.url.trim_end_matches('/'));
    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            results.push(CheckResult::new(
                CheckStatus::Fail,
                format!("ollama unreachable: cannot initialize HTTP client: {err}"),
            ));
            return results;
        }
    };

    match http.get(&tags_url).send().await {
        Ok(resp) if resp.status() == StatusCode::OK => {
            match resp.json::<OllamaTagsResponse>().await {
                Ok(body) => {
                    let available: BTreeSet<String> =
                        body.models.into_iter().map(|m| m.name).collect();
                    results.push(CheckResult::new(
                        CheckStatus::Ok,
                        format!("ollama reachable: {} models available", available.len()),
                    ));

                    let missing: Vec<_> = configured_models
                        .iter()
                        .filter(|model| !available.contains(model.as_str()))
                        .cloned()
                        .collect();

                    if missing.is_empty() {
                        results.push(CheckResult::new(
                            CheckStatus::Ok,
                            "configured models available in ollama",
                        ));
                    } else {
                        results.push(CheckResult::new(
                            CheckStatus::Fail,
                            format!("missing models in ollama: {}", missing.join(", ")),
                        ));
                    }
                }
                Err(err) => {
                    results.push(CheckResult::new(
                        CheckStatus::Fail,
                        format!("ollama tags parse failed: {err}"),
                    ));
                }
            }
        }
        Ok(resp) => {
            results.push(CheckResult::new(
                CheckStatus::Fail,
                format!("ollama tags endpoint returned status {}", resp.status()),
            ));
        }
        Err(err) => {
            results.push(CheckResult::new(
                CheckStatus::Fail,
                format!("ollama unreachable: {err}"),
            ));
        }
    }

    results
}

fn configured_models(config: &AppConfig) -> Vec<String> {
    let mut models = BTreeSet::new();
    models.insert(config.llm.model.clone());
    models.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::Daemon;
    use crate::system_adapter::fake::FakeSystemAdapter;
    use std::sync::{Mutex, OnceLock};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::watch;
    use tokio::time::{Duration, sleep};

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn test_config(ollama_url: String, model: &str) -> AppConfig {
        AppConfig {
            llm: crate::config::LlmConfig {
                backend: crate::config::Backend::Ollama,
                url: ollama_url,
                model: model.to_owned(),
                keep_alive_secs: 300,
            },
        }
    }

    async fn spawn_tags_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener
            .local_addr()
            .expect("listener should have local addr");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("server should accept");
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("server should write response");
        });
        format!("http://{addr}")
    }

    async fn spawn_daemon(socket_path: PathBuf, config: AppConfig) -> watch::Sender<bool> {
        let daemon = Daemon::new_with_socket_path(config, socket_path.clone())
            .expect("daemon should initialize");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            let _ = daemon.run(shutdown_rx).await;
        });

        for _ in 0..20 {
            if socket_path.exists() {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        shutdown_tx
    }

    #[tokio::test]
    async fn missing_socket_produces_single_fail_result() {
        let _guard = test_lock().lock().expect("test mutex poisoned");
        let temp = tempfile::tempdir().expect("temp dir should exist");
        let socket_path = temp.path().join("missing.sock");
        let ollama_url = spawn_tags_server(r#"{"models":[{"name":"llama3.2:1b"}]}"#).await;
        let config = test_config(ollama_url, "llama3.2:1b");
        let transport = DaemonTransport::new(socket_path.clone());
        let adapter = FakeSystemAdapter::new(true, true);

        let results = run(&config, &transport, socket_path.clone(), &adapter).await;

        let fail_results: Vec<_> = results
            .iter()
            .filter(|result| result.status == CheckStatus::Fail)
            .collect();
        assert_eq!(fail_results.len(), 1, "only the missing socket should fail");
        assert!(
            fail_results[0]
                .message
                .contains(socket_path.to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn unreachable_daemon_produces_fail_result() {
        let _guard = test_lock().lock().expect("test mutex poisoned");
        let temp = tempfile::tempdir().expect("temp dir should exist");
        let socket_path = temp.path().join("stale.sock");
        std::fs::write(&socket_path, "not a socket")
            .expect("stale socket marker should be written");
        let ollama_url = spawn_tags_server(r#"{"models":[{"name":"llama3.2:1b"}]}"#).await;
        let config = test_config(ollama_url, "llama3.2:1b");
        let transport = DaemonTransport::new(socket_path);
        let adapter = FakeSystemAdapter::new(true, true);

        let results = run(
            &config,
            &transport,
            temp.path().join("stale.sock"),
            &adapter,
        )
        .await;

        assert!(results.iter().any(|result| {
            result.status == CheckStatus::Fail && result.message.starts_with("daemon unreachable:")
        }));
    }

    #[tokio::test]
    async fn unreachable_ollama_produces_fail_result() {
        let _guard = test_lock().lock().expect("test mutex poisoned");
        let temp = tempfile::tempdir().expect("temp dir should exist");
        let socket_path = temp.path().join("doctor.sock");
        let config = test_config("http://127.0.0.1:9".to_owned(), "llama3.2:1b");
        let shutdown_tx = spawn_daemon(socket_path.clone(), config.clone()).await;
        let transport = DaemonTransport::new(socket_path);
        let adapter = FakeSystemAdapter::new(true, false);

        let results = run(
            &config,
            &transport,
            temp.path().join("doctor.sock"),
            &adapter,
        )
        .await;

        assert!(results.iter().any(|result| {
            result.status == CheckStatus::Fail
                && result.message == "ollama unreachable: health check failed"
        }));
        let _ = shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn missing_model_produces_fail_result() {
        let _guard = test_lock().lock().expect("test mutex poisoned");
        let temp = tempfile::tempdir().expect("temp dir should exist");
        let socket_path = temp.path().join("doctor.sock");
        let ollama_url = spawn_tags_server(r#"{"models":[{"name":"different-model"}]}"#).await;
        let config = test_config(ollama_url, "llama3.2:1b");
        let shutdown_tx = spawn_daemon(socket_path.clone(), config.clone()).await;
        let transport = DaemonTransport::new(socket_path.clone());
        let adapter = FakeSystemAdapter::new(true, true);

        let results = run(&config, &transport, socket_path, &adapter).await;

        assert!(results.iter().any(|result| {
            result.status == CheckStatus::Fail
                && result.message == "missing models in ollama: llama3.2:1b"
        }));
        let _ = shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn all_checks_pass_without_failures() {
        let _guard = test_lock().lock().expect("test mutex poisoned");
        let temp = tempfile::tempdir().expect("temp dir should exist");
        let socket_path = temp.path().join("doctor.sock");
        let ollama_url = spawn_tags_server(r#"{"models":[{"name":"llama3.2:1b"}]}"#).await;
        let config = test_config(ollama_url, "llama3.2:1b");
        let shutdown_tx = spawn_daemon(socket_path.clone(), config.clone()).await;
        let transport = DaemonTransport::new(socket_path.clone());
        let adapter = FakeSystemAdapter::new(true, true);

        let results = run(&config, &transport, socket_path, &adapter).await;

        assert!(
            results
                .iter()
                .all(|result| result.status != CheckStatus::Fail),
            "no checks should fail: {results:?}"
        );
        let _ = shutdown_tx.send(true);
    }
}
