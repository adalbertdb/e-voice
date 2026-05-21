//! Configuration loading, defaults, and persistent daemon state paths.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const DEFAULT_CONFIG_TOML: &str = include_str!("../config/default.toml");

/// Which LLM backend to use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Ollama,
    LmStudio,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub backend: Backend,
    pub url: String,
    pub model: String,
    /// Seconds to keep the model loaded after a request. `-1` means keep forever.
    #[serde(default = "default_keep_alive_secs")]
    pub keep_alive_secs: i64,
}

fn default_keep_alive_secs() -> i64 {
    300
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub llm: LlmConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG_TOML).expect("embedded default config must be valid")
    }
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_file_path()?;
        if path.exists() {
            return Self::from_file(path);
        }

        Ok(Self::default())
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Ok(toml::from_str(&content)?)
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse/serialize error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("$HOME is not set")]
    MissingHome,
}

pub fn config_dir() -> Result<PathBuf, ConfigError> {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path).join("e-voice"));
    }

    let home = std::env::var("HOME").map_err(|_| ConfigError::MissingHome)?;
    Ok(PathBuf::from(home).join(".config").join("e-voice"))
}

pub fn config_file_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, Backend};

    #[test]
    fn default_config_loads() {
        let cfg = AppConfig::default();
        assert!(!cfg.llm.url.is_empty());
        assert!(!cfg.llm.model.is_empty());
        assert_eq!(cfg.llm.backend, Backend::Ollama);
        assert_eq!(cfg.llm.keep_alive_secs, 300);
    }

    #[test]
    fn parses_toml_into_config() {
        let input = r#"
[llm]
backend = "ollama"
url = "http://localhost:11434"
model = "llama3.2:1b"
keep_alive_secs = 60
"#;

        let cfg: AppConfig = toml::from_str(input).expect("valid config TOML should parse");
        assert_eq!(cfg.llm.url, "http://localhost:11434");
        assert_eq!(cfg.llm.model, "llama3.2:1b");
        assert_eq!(cfg.llm.keep_alive_secs, 60);
    }

    #[test]
    fn parses_lm_studio_backend() {
        let input = r#"
[llm]
backend = "lm_studio"
url = "http://localhost:1234"
model = "lmstudio-community/meta-llama-3.1-8b-instruct-gguf"
keep_alive_secs = 120
"#;

        let cfg: AppConfig = toml::from_str(input).expect("lm_studio config should parse");
        assert_eq!(cfg.llm.backend, Backend::LmStudio);
        assert_eq!(cfg.llm.url, "http://localhost:1234");
    }
}
