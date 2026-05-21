//! Configuration loading, defaults, and persistent daemon state paths.

use crate::modes::Profile;
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
}

/// Persistent runtime state that survives daemon restarts.
///
/// Stored as TOML at [`state_file_path()`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PersistentState {
    /// The active profile name.
    pub profile: Profile,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            profile: Profile::default(),
        }
    }
}

impl PersistentState {
    /// Load from the state file, falling back to [`Default`] if the file does
    /// not exist or cannot be parsed.
    pub fn load() -> Self {
        match state_file_path() {
            Ok(path) if path.exists() => Self::from_file(&path).unwrap_or_default(),
            _ => Self::default(),
        }
    }

    /// Load from an explicit path (used in tests and by [`load`]).
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path.as_ref())?;
        Ok(toml::from_str(&content)?)
    }

    /// Persist to the state file.  Creates parent directories if needed.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = state_file_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)?;
        fs::write(path, body)?;
        Ok(())
    }

    /// Persist to an explicit path (used in tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)?;
        fs::write(path, body)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{AppConfig, PersistentState};
    use crate::modes::Profile;
    use tempfile::tempdir;

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

    #[test]
    fn test_active_profile_persists_to_state_file() {
        let dir = tempdir().expect("tempdir should be created");
        let state_path = dir.path().join("state.toml");

        let state = PersistentState {
            profile: Profile::Formal,
        };
        state
            .save_to(&state_path)
            .expect("state should be saved");

        let loaded =
            PersistentState::from_file(&state_path).expect("state should be loaded");
        assert_eq!(
            loaded.profile,
            Profile::Formal,
            "loaded profile should match saved profile"
        );
    }

    #[test]
    fn test_default_profile_is_universal_interpreter() {
        let state = PersistentState::default();
        assert_eq!(
            state.profile,
            Profile::UniversalInterpreter,
            "default profile should be UniversalInterpreter"
        );
    }

    #[test]
    fn test_persistent_state_roundtrip_all_profiles() {
        let dir = tempdir().expect("tempdir should be created");
        let profiles = vec![
            Profile::UniversalInterpreter,
            Profile::Formal,
            Profile::Casual,
            Profile::Bullet,
            Profile::Translate("ja".to_owned()),
        ];
        for profile in profiles {
            let state_path = dir.path().join(format!("state_{}.toml", profile.name()));
            let state = PersistentState {
                profile: profile.clone(),
            };
            state.save_to(&state_path).expect("should save");
            let loaded = PersistentState::from_file(&state_path).expect("should load");
            assert_eq!(loaded.profile, profile);
        }
    }
}
