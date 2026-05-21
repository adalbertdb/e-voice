//! Ollama-backed text processor. Prompt construction is an internal detail.

use crate::config::AppConfig;
use crate::modes::Mode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};

const OLLAMA_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone)]
pub struct TextProcessor {
    client: Client,
    config: AppConfig,
}

impl TextProcessor {
    pub fn new(config: AppConfig) -> Result<Self, ProcessorError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(OLLAMA_TIMEOUT_SECS))
            .build()?;

        Ok(Self { client, config })
    }

    pub fn config_model(&self) -> &str {
        &self.config.ollama.model
    }

    pub async fn list_models(&self) -> Result<Vec<String>, ProcessorError> {
        let url = format!("{}/api/tags", self.config.ollama.url.trim_end_matches('/'));
        let response = self.client.get(&url).send().await?.error_for_status()?;
        #[derive(Deserialize)]
        struct TagsResponse {
            models: Vec<TagEntry>,
        }
        #[derive(Deserialize)]
        struct TagEntry {
            name: String,
        }
        let body: TagsResponse = response.json().await?;
        Ok(body.models.into_iter().map(|m| m.name).collect())
    }

    pub async fn process(
        &self,
        raw_text: &str,
        request_id: &str,
        override_model: Option<&str>,
    ) -> String {
        match self
            .process_inner(raw_text, request_id, override_model)
            .await
        {
            Ok(output) if !output.trim().is_empty() => output,
            Ok(_) => {
                warn!(request_id = %request_id, "processor returned empty output, falling back to raw text");
                raw_text.to_owned()
            }
            Err(err) => {
                warn!(
                    request_id = %request_id,
                    error = %err,
                    error_kind = %err.kind(),
                    "processor failed, falling back to raw text"
                );
                raw_text.to_owned()
            }
        }
    }

    async fn process_inner(
        &self,
        raw_text: &str,
        request_id: &str,
        override_model: Option<&str>,
    ) -> Result<String, ProcessorError> {
        let endpoint = format!(
            "{}/api/generate",
            self.config.ollama.url.trim_end_matches('/')
        );
        let model = override_model
            .unwrap_or(&self.config.ollama.model)
            .to_owned();
        let payload = OllamaGenerateRequest {
            model: model.clone(),
            prompt: Mode::Clean.prompt_template(raw_text),
            stream: false,
        };

        info!(
            request_id = %request_id,
            model = %model,
            endpoint = %endpoint,
            input_len = raw_text.len(),
            "sending request to ollama"
        );

        let response = self
            .client
            .post(endpoint)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        let body: OllamaGenerateResponse = response.json().await?;
        let trimmed = body.response.trim().to_owned();
        if trimmed.is_empty() {
            return Err(ProcessorError::EmptyResponse);
        }

        debug!(request_id = %request_id, output_len = trimmed.len(), "ollama request completed");
        Ok(trimmed)
    }
}

#[derive(Debug, Serialize)]
struct OllamaGenerateRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}

#[derive(Debug, Error)]
pub enum ProcessorError {
    #[error("http client error: {0}")]
    HttpClient(#[from] reqwest::Error),
    #[error("empty response from ollama")]
    EmptyResponse,
}

impl ProcessorError {
    pub fn kind(&self) -> &'static str {
        match self {
            ProcessorError::HttpClient(err) if err.is_timeout() => "ollama_timeout",
            ProcessorError::HttpClient(err) if err.is_status() => "ollama_http_error",
            ProcessorError::HttpClient(_) => "ollama_transport_error",
            ProcessorError::EmptyResponse => "empty_model_response",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessorError;

    #[test]
    fn classifies_empty_response_error_kind() {
        let err = ProcessorError::EmptyResponse;
        assert_eq!(err.kind(), "empty_model_response");
    }
}
