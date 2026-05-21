//! Shallow orchestrator: builds the prompt and delegates to an `LlmBackend`.
//!
//! Prompt construction is an internal detail — callers do not influence it.

use crate::config::AppConfig;
use crate::llm_backend::{BackendError, LlmBackend, from_config};
use crate::modes::Profile;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Re-export so that existing callers of `crate::processor::ProcessorError` continue to work.
pub type ProcessorError = BackendError;

pub struct TextProcessor {
    backend: Arc<dyn LlmBackend>,
    default_model: String,
    keep_alive_secs: i64,
}

impl std::fmt::Debug for TextProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextProcessor")
            .field("default_model", &self.default_model)
            .field("keep_alive_secs", &self.keep_alive_secs)
            .finish_non_exhaustive()
    }
}

impl Clone for TextProcessor {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            default_model: self.default_model.clone(),
            keep_alive_secs: self.keep_alive_secs,
        }
    }
}

impl TextProcessor {
    pub fn new(config: AppConfig) -> Result<Self, ProcessorError> {
        let backend = from_config(&config)?;
        Ok(Self {
            backend: Arc::from(backend),
            default_model: config.llm.model.clone(),
            keep_alive_secs: config.llm.keep_alive_secs,
        })
    }

    /// Construct with an explicit backend (useful for tests).
    pub fn with_backend(
        backend: impl LlmBackend + 'static,
        default_model: impl Into<String>,
        keep_alive_secs: i64,
    ) -> Self {
        Self {
            backend: Arc::new(backend),
            default_model: default_model.into(),
            keep_alive_secs,
        }
    }

    pub fn config_model(&self) -> &str {
        &self.default_model
    }

    pub async fn list_models(&self) -> Result<Vec<String>, ProcessorError> {
        self.backend.list_models().await
    }

    pub async fn process(
        &self,
        raw_text: &str,
        request_id: &str,
        override_model: Option<&str>,
        profile: &Profile,
    ) -> String {
        match self
            .process_inner(raw_text, request_id, override_model, profile)
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
        profile: &Profile,
    ) -> Result<String, ProcessorError> {
        let model = override_model.unwrap_or(&self.default_model);
        let prompt = profile.prompt_for(raw_text);

        info!(
            request_id = %request_id,
            model = %model,
            input_len = raw_text.len(),
            "sending request to llm backend"
        );

        let result = self
            .backend
            .process(model, &prompt, self.keep_alive_secs)
            .await?;

        debug!(request_id = %request_id, output_len = result.len(), "llm backend request completed");
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_backend::BackendError;
    use async_trait::async_trait;

    struct MockBackend {
        response: Result<String, ()>,
    }

    #[async_trait]
    impl LlmBackend for MockBackend {
        async fn load_model(&self, _model: &str) -> Result<(), BackendError> {
            Ok(())
        }
        async fn process(
            &self,
            _model: &str,
            _prompt: &str,
            _keep_alive_secs: i64,
        ) -> Result<String, BackendError> {
            self.response
                .as_ref()
                .map(|s| s.clone())
                .map_err(|_| BackendError::EmptyResponse)
        }
        async fn unload_model(&self, _model: &str) -> Result<(), BackendError> {
            Ok(())
        }
        async fn list_models(&self) -> Result<Vec<String>, BackendError> {
            Ok(vec!["model-a".to_owned()])
        }
        async fn health_check(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_processor_calls_backend_process() {
        let backend = MockBackend {
            response: Ok("processed output".to_owned()),
        };
        let processor = TextProcessor::with_backend(backend, "test-model", 300);
        let result = processor
            .process("raw input", "req-1", None, &Profile::default())
            .await;
        assert_eq!(result, "processed output");
    }

    #[tokio::test]
    async fn test_processor_fallback_on_empty_response() {
        let backend = MockBackend {
            response: Err(()),
        };
        let processor = TextProcessor::with_backend(backend, "test-model", 300);
        let raw = "raw input text";
        let result = processor
            .process(raw, "req-2", None, &Profile::default())
            .await;
        // Backend error → fallback to raw input.
        assert_eq!(result, raw);
    }

    struct ModelCapturingBackend {
        captured_model: std::sync::Arc<tokio::sync::Mutex<String>>,
    }

    #[async_trait]
    impl LlmBackend for ModelCapturingBackend {
        async fn load_model(&self, _model: &str) -> Result<(), BackendError> {
            Ok(())
        }
        async fn process(
            &self,
            model: &str,
            _prompt: &str,
            _keep_alive_secs: i64,
        ) -> Result<String, BackendError> {
            *self.captured_model.lock().await = model.to_owned();
            Ok("output".to_owned())
        }
        async fn unload_model(&self, _model: &str) -> Result<(), BackendError> {
            Ok(())
        }
        async fn list_models(&self) -> Result<Vec<String>, BackendError> {
            Ok(vec![])
        }
        async fn health_check(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_processor_override_model_is_forwarded_to_backend() {
        let captured = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
        let backend = ModelCapturingBackend {
            captured_model: Arc::clone(&captured),
        };
        let processor = TextProcessor::with_backend(backend, "default-model", 300);
        let result = processor
            .process("raw input", "req-3", Some("override-model"), &Profile::default())
            .await;
        assert_eq!(result, "output");
        assert_eq!(*captured.lock().await, "override-model");
    }

    #[test]
    fn classifies_empty_response_error_kind() {
        let err = BackendError::EmptyResponse;
        assert_eq!(err.kind(), "empty_model_response");
    }
}
