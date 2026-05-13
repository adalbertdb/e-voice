//! Configuration loading, defaults, and persistent daemon state paths.

use crate::modes::Mode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const DEFAULT_CONFIG_TOML: &str = include_str!("../config/default.toml");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub ollama: OllamaConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OllamaConfig {
    pub url: String,
    pub model: String,
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

    pub fn model_for_mode(&self, _mode: &Mode) -> &str {
        &self.ollama.model
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
    use super::AppConfig;

    #[test]
    fn default_config_loads() {
        let cfg = AppConfig::default();
        assert!(!cfg.ollama.url.is_empty());
        assert!(!cfg.ollama.model.is_empty());
    }

    #[test]
    fn parses_toml_into_config() {
        let input = r#"
[ollama]
url = "http://localhost:11434"
model = "llama3.2:1b"
"#;

        let cfg: AppConfig = toml::from_str(input).expect("valid config TOML should parse");
        assert_eq!(cfg.ollama.url, "http://localhost:11434");
        assert_eq!(cfg.ollama.model, "llama3.2:1b");
    }
}
