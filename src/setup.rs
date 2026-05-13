//! First-run setup wizard for voxtype, Ollama, and Omarchy snippets.

use crate::config::{AppConfig, config_dir, config_file_path};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

const USER_SERVICE_NAME: &str = "e-voice.service";
const USER_SERVICE_CONTENT: &str = include_str!("../packaging/e-voice.service");

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

const POST_PROCESS_BLOCK: &str = r#"[output.post_process]
command = "e-voice process"
timeout_ms = 10000
"#;

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

pub fn run_setup() -> Result<(), SetupError> {
    info("Step 1: checking voxtype installation");
    check_binary("voxtype")?;

    info("Step 2: patching voxtype config");
    patch_voxtype_config()?;

    info("Step 3: checking ollama installation");
    check_binary("ollama")?;

    info("Step 4: pulling required models");
    run_command("ollama", &["pull", "llama3.2:1b"])?;
    success("Pulled model llama3.2:1b");
    run_command("ollama", &["pull", "qwen2.5:1.5b"])?;
    success("Pulled model qwen2.5:1.5b");

    info("Step 5: ensuring e-voice config exists");
    ensure_evoice_config()?;

    info("Step 6: generating Hyprland snippet");
    write_snippet("hyprland-snippet.conf", HYPRLAND_SNIPPET)?;

    info("Step 7: generating Waybar snippet");
    write_snippet("waybar-snippet.json", WAYBAR_SNIPPET)?;

    info("Step 8: enabling user service if available");
    maybe_enable_service()?;

    success("Setup completed");
    Ok(())
}

fn patch_voxtype_config() -> Result<(), SetupError> {
    let voxtype_config = voxtype_config_path()?;
    if let Some(parent) = voxtype_config.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut content = if voxtype_config.exists() {
        fs::read_to_string(&voxtype_config)?
    } else {
        String::new()
    };

    if content.contains("[output.post_process]") && content.contains("e-voice process") {
        info("voxtype post_process hook already configured");
        return Ok(());
    }

    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(POST_PROCESS_BLOCK);
    fs::write(voxtype_config, content)?;
    success("Patched voxtype config with post_process hook");
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

fn maybe_enable_service() -> Result<(), SetupError> {
    let service_path = expand_home(&format!(".config/systemd/user/{USER_SERVICE_NAME}"))?;
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&service_path, USER_SERVICE_CONTENT)?;
    success(&format!(
        "Installed user service to {}",
        service_path.display()
    ));

    run_command("systemctl", &["--user", "daemon-reload"])?;
    run_command("systemctl", &["--user", "enable", "--now", "e-voice"])?;
    success("Enabled and started e-voice user service");
    Ok(())
}

fn check_binary(binary: &str) -> Result<(), SetupError> {
    let status = Command::new("which").arg(binary).status()?;
    if status.success() {
        success(&format!("Found {binary}"));
        Ok(())
    } else {
        failure(&format!("{binary} is not installed"));
        Err(SetupError::MissingDependency(binary.to_owned()))
    }
}

fn run_command(program: &str, args: &[&str]) -> Result<(), SetupError> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(SetupError::CommandFailed(format!(
            "{program} {}",
            args.join(" ")
        )))
    }
}

fn voxtype_config_path() -> Result<PathBuf, SetupError> {
    expand_home(".config/voxtype/config.toml")
}

fn expand_home(path: &str) -> Result<PathBuf, SetupError> {
    let home = std::env::var("HOME").map_err(|_| SetupError::MissingHome)?;
    Ok(Path::new(&home).join(path))
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
    #[error("command failed: {0}")]
    CommandFailed(String),
    #[error("$HOME is not set")]
    MissingHome,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::ser::Error),
    #[error("config error: {0}")]
    Config(String),
}
