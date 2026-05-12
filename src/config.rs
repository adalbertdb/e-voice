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
    pub models: ModelsConfig,
    pub mode: ModeConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OllamaConfig {
    pub url: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsConfig {
    pub clean: String,
    pub formal: String,
    pub translate: String,
    #[serde(default = "default_casual_model")]
    pub casual: String,
    #[serde(default = "default_bullet_model")]
    pub bullet: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModeConfig {
    pub default: String,
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

    pub fn model_for_mode(&self, mode: &Mode) -> &str {
        match mode {
            Mode::Clean => &self.models.clean,
            Mode::Formal => &self.models.formal,
            Mode::Casual => &self.models.casual,
            Mode::Bullet => &self.models.bullet,
            Mode::Translate(_) => &self.models.translate,
        }
    }

    pub fn default_mode(&self) -> Mode {
        self.mode.default.parse().unwrap_or(Mode::Clean)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateFile {
    pub mode: String,
}

impl StateFile {
    pub fn from_mode(mode: &Mode) -> Self {
        Self {
            mode: mode.to_string(),
        }
    }

    pub fn to_mode(&self) -> Mode {
        self.mode.parse().unwrap_or(Mode::Clean)
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

pub fn state_file_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("state.toml"))
}

pub fn load_state(default_mode: &Mode) -> Result<Mode, ConfigError> {
    let path = state_file_path()?;
    if !path.exists() {
        return Ok(default_mode.clone());
    }

    let content = fs::read_to_string(path)?;
    let state: StateFile = toml::from_str(&content)?;
    Ok(state.to_mode())
}

pub fn save_state(mode: &Mode) -> Result<(), ConfigError> {
    let path = state_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let state = StateFile::from_mode(mode);
    let body = toml::to_string(&state)?;
    fs::write(path, body)?;
    Ok(())
}

fn default_casual_model() -> String {
    "llama3.2:1b".to_string()
}

fn default_bullet_model() -> String {
    "llama3.2:1b".to_string()
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

[models]
clean = "llama3.2:1b"
formal = "llama3.2:1b"
casual = "llama3.2:1b"
bullet = "llama3.2:1b"
translate = "qwen2.5:1.5b"

[mode]
default = "clean"
"#;

        let cfg: AppConfig = toml::from_str(input).expect("valid config TOML should parse");
        assert_eq!(cfg.ollama.url, "http://localhost:11434");
        assert_eq!(cfg.mode.default, "clean");
        assert_eq!(cfg.models.translate, "qwen2.5:1.5b");
    }
}
