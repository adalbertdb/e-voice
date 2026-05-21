//! First-run setup wizard for voxtype, Ollama, and Omarchy snippets.

use crate::config::{config_dir, config_file_path, AppConfig};
use crate::system_adapter::SystemAdapter;
use std::fs;
use thiserror::Error;

const USER_SERVICE_CONTENT: &str = include_str!("../packaging/e-voice.service");

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

const HYPRLAND_SNIPPET: &str = r#"# e-voice / voxtype keybindings
# Paste into ~/.config/hypr/bindings.conf
bind = , F9, exec, voxtype record start
bindr = , F9, exec, voxtype record stop
"#;

const WAYBAR_SNIPPET: &str = r#""custom/e-voice": {
  "exec": "e-voice status --follow --format json",
  "return-type": "json",
  "format": "{}",
  "tooltip": true
}
"#;

pub fn run_setup<A: SystemAdapter>(adapter: &A) -> Result<(), SetupError> {
    info("Step 1: checking voxtype installation");
    if !adapter.is_binary_available("voxtype") {
        failure("voxtype is not installed");
        return Err(SetupError::MissingDependency("voxtype".to_owned()));
    }
    success("Found voxtype");

    info("Step 2: patching voxtype config");
    adapter
        .patch_voxtype_post_process_hook()
        .map_err(|e| SetupError::Adapter(e.to_string()))?;
    success("voxtype post_process hook configured");

    info("Step 3: checking ollama installation");
    if !adapter.is_binary_available("ollama") {
        failure("ollama is not installed");
        return Err(SetupError::MissingDependency("ollama".to_owned()));
    }
    success("Found ollama");

    info("Step 4: pulling required models");
    adapter
        .pull_ollama_model("llama3.2:1b")
        .map_err(|e| SetupError::Adapter(e.to_string()))?;
    success("Pulled model llama3.2:1b");
    adapter
        .pull_ollama_model("qwen2.5:1.5b")
        .map_err(|e| SetupError::Adapter(e.to_string()))?;
    success("Pulled model qwen2.5:1.5b");

    info("Step 5: ensuring e-voice config exists");
    ensure_evoice_config()?;

    info("Step 6: generating Hyprland snippet");
    write_snippet("hyprland-snippet.conf", HYPRLAND_SNIPPET)?;

    info("Step 7: generating Waybar snippet");
    write_snippet("waybar-snippet.json", WAYBAR_SNIPPET)?;

    info("Step 8: enabling user service if available");
    adapter
        .enable_autostart(USER_SERVICE_CONTENT)
        .map_err(|e| SetupError::Adapter(e.to_string()))?;
    success("Enabled and started e-voice user service");

    success("Setup completed");
    Ok(())
}

fn ensure_evoice_config() -> Result<(), SetupError> {
    let dir = config_dir().map_err(|e| SetupError::Config(e.to_string()))?;
    fs::create_dir_all(&dir)?;

    let config_path = config_file_path().map_err(|e| SetupError::Config(e.to_string()))?;
    if config_path.exists() {
        info("e-voice config already exists");
        return Ok(());
    }

    let default_cfg = AppConfig::default();
    let body = toml::to_string_pretty(&default_cfg)?;
    fs::write(config_path, body)?;
    success("Created default e-voice config.toml");
    Ok(())
}

fn write_snippet(file_name: &str, content: &str) -> Result<(), SetupError> {
    let dir = config_dir().map_err(|e| SetupError::Config(e.to_string()))?;
    fs::create_dir_all(&dir)?;
    let path = dir.join(file_name);
    fs::write(path, content)?;
    success(&format!("Wrote {file_name}"));
    Ok(())
}

fn success(message: &str) {
    println!("{GREEN}✓{RESET} {message}");
}

fn failure(message: &str) {
    println!("{RED}✗{RESET} {message}");
}

fn info(message: &str) {
    println!("{YELLOW}→{RESET} {message}");
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("missing dependency: {0}")]
    MissingDependency(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::ser::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("adapter error: {0}")]
    Adapter(String),
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_adapter::fake::FakeSystemAdapter;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn enable_autostart_called_exactly_once_during_clean_setup() {
        let _guard = test_lock().lock().expect("test mutex poisoned");

        let temp = tempdir().expect("failed to create temp dir");
        // SAFETY: tests are serialized via global mutex, so process-wide env mutation is controlled.
        unsafe {
            std::env::set_var("HOME", temp.path());
        }

        let adapter = FakeSystemAdapter::new(true, true);
        let _ = run_setup(&adapter);

        let calls = adapter.calls();
        let count = calls.iter().filter(|c| *c == "enable_autostart").count();
        assert_eq!(count, 1, "enable_autostart should be called exactly once");
    }
}
