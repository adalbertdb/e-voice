//! Platform abstraction for all OS-level operations.
//!
//! [`SystemAdapter`] defines the seam between business logic and the host OS.
//! [`LinuxSystemAdapter`] provides the concrete Linux implementation extracted
//! verbatim from the original `tray.rs` and `setup.rs` code.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Result alias used by every [`SystemAdapter`] method.
pub type AdapterResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// (label, model, language) presets for the STT language picker.
pub const STT_PRESETS: [(&str, &str, &str); 6] = [
    ("English", "base.en", "en"),
    ("Spanish", "base", "es"),
    ("French", "base", "fr"),
    ("Portuguese", "base", "pt"),
    ("German", "base", "de"),
    ("Italian", "base", "it"),
];

const POST_PROCESS_BLOCK: &str = "[output.post_process]\ncommand = \"e-voice process\"\ntimeout_ms = 10000\n";

/// Abstracts all platform-specific OS operations so that callers can be tested
/// with a [`FakeSystemAdapter`] without touching the real filesystem or running
/// real processes.
pub trait SystemAdapter {
    /// Restart the voxtype STT background service.
    fn restart_stt_service(&self) -> AdapterResult<()>;

    /// Return `true` if the voxtype STT service is currently active.
    fn is_stt_service_running(&self) -> bool;

    /// Ask the service manager to reload its unit files (daemon-reload).
    fn reload_service_manager(&self) -> AdapterResult<()>;

    /// Install and enable the e-voice user service with the given unit content.
    fn enable_autostart(&self, service_content: &str) -> AdapterResult<()>;

    /// Return `true` if the Ollama HTTP API at `url` is reachable.
    async fn check_ollama_health(&self, url: &str) -> bool;

    /// Open `path` with the system default application (e.g. xdg-open).
    fn open_file(&self, path: &Path) -> AdapterResult<()>;

    /// Send a desktop notification.
    fn send_notification(&self, summary: &str, body: &str) -> AdapterResult<()>;

    /// Read the current STT preset from the voxtype config.
    ///
    /// Returns `(label, model, language)`.
    fn read_voxtype_stt_config(&self) -> AdapterResult<(String, String, String)>;

    /// Overwrite the `[whisper]` section in the voxtype config with new values.
    fn write_voxtype_stt_config(&self, model: &str, language: &str) -> AdapterResult<()>;

    /// Append the e-voice `[output.post_process]` hook to the voxtype config if
    /// it is not already present.
    fn patch_voxtype_post_process_hook(&self) -> AdapterResult<()>;

    /// Absolute path to the voxtype `config.toml`.
    fn voxtype_config_path(&self) -> PathBuf;

    /// Return `true` if `name` is available on `$PATH`.
    fn is_binary_available(&self, name: &str) -> bool;

    /// Pull an Ollama model by name (blocking).
    fn pull_ollama_model(&self, model: &str) -> AdapterResult<()>;
}

// ─── Linux implementation ────────────────────────────────────────────────────

/// Concrete [`SystemAdapter`] for Linux using systemctl, xdg-open, notify-rust,
/// and reqwest.
pub struct LinuxSystemAdapter;

impl SystemAdapter for LinuxSystemAdapter {
    fn restart_stt_service(&self) -> AdapterResult<()> {
        std::process::Command::new("systemctl")
            .args(["--user", "restart", "voxtype.service"])
            .status()?;
        Ok(())
    }

    fn is_stt_service_running(&self) -> bool {
        std::process::Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", "voxtype.service"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn reload_service_manager(&self) -> AdapterResult<()> {
        std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()?;
        Ok(())
    }

    fn enable_autostart(&self, service_content: &str) -> AdapterResult<()> {
        let home = std::env::var("HOME")?;
        let service_path =
            PathBuf::from(home).join(".config/systemd/user/e-voice.service");
        if let Some(parent) = service_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&service_path, service_content)?;
        self.reload_service_manager()?;
        std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "e-voice"])
            .status()?;
        Ok(())
    }

    async fn check_ollama_health(&self, url: &str) -> bool {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
        else {
            return false;
        };
        let check_url = format!("{}/api/tags", url.trim_end_matches('/'));
        client
            .get(&check_url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    fn open_file(&self, path: &Path) -> AdapterResult<()> {
        std::process::Command::new("xdg-open").arg(path).status()?;
        Ok(())
    }

    fn send_notification(&self, summary: &str, body: &str) -> AdapterResult<()> {
        let summary = summary.to_owned();
        let body = body.to_owned();
        std::thread::spawn(move || {
            let _ = notify_rust::Notification::new()
                .summary(&summary)
                .body(&body)
                .icon("e-voice")
                .timeout(notify_rust::Timeout::Milliseconds(3000))
                .show();
        });
        Ok(())
    }

    fn read_voxtype_stt_config(&self) -> AdapterResult<(String, String, String)> {
        read_voxtype_stt_config_from_path(&self.voxtype_config_path())
    }

    fn write_voxtype_stt_config(&self, model: &str, language: &str) -> AdapterResult<()> {
        write_voxtype_stt_config_to_path(&self.voxtype_config_path(), model, language)
    }

    fn patch_voxtype_post_process_hook(&self) -> AdapterResult<()> {
        patch_voxtype_post_process_hook_at_path(&self.voxtype_config_path())
    }

    fn voxtype_config_path(&self) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".config/voxtype/config.toml")
    }

    fn is_binary_available(&self, name: &str) -> bool {
        std::process::Command::new("which")
            .arg(name)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn pull_ollama_model(&self, model: &str) -> AdapterResult<()> {
        let status = std::process::Command::new("ollama")
            .args(["pull", model])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("ollama pull {model} failed").into())
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn extract_toml_value_in_whisper(content: &str, key: &str) -> Option<String> {
    let mut in_whisper = false;
    let prefix = format!("{key} = \"");
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_whisper = trimmed == "[whisper]";
            continue;
        }

        if in_whisper
            && trimmed.starts_with(&prefix)
            && let Some(start) = trimmed.find('"')
            && let Some(end) = trimmed[start + 1..].find('"')
        {
            return Some(trimmed[start + 1..start + 1 + end].to_owned());
        }
    }
    None
}

fn read_voxtype_stt_config_from_path(path: &Path) -> AdapterResult<(String, String, String)> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(("Unknown".into(), "base.en".into(), "en".into())),
    };

    let model = extract_toml_value_in_whisper(&content, "model")
        .unwrap_or_else(|| "base.en".into());
    let language = extract_toml_value_in_whisper(&content, "language")
        .unwrap_or_else(|| "en".into());

    let label = STT_PRESETS
        .iter()
        .find(|(_, m, l)| *m == model && *l == language)
        .map(|(lbl, _, _)| lbl.to_string())
        .unwrap_or_else(|| language.clone());

    Ok((label, model, language))
}

fn write_voxtype_stt_config_to_path(
    path: &Path,
    model: &str,
    language: &str,
) -> AdapterResult<()> {
    let content = std::fs::read_to_string(path).unwrap_or_default();

    let mut new_lines: Vec<String> = Vec::new();
    let mut in_whisper = false;
    let mut model_set = false;
    let mut language_set = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_whisper {
                if !model_set {
                    new_lines.push(format!("model = \"{model}\""));
                }
                if !language_set {
                    new_lines.push(format!("language = \"{language}\""));
                }
            }
            in_whisper = trimmed == "[whisper]";
            new_lines.push(line.to_owned());
            continue;
        }

        if in_whisper && trimmed.starts_with("model = \"") {
            new_lines.push(format!("model = \"{model}\""));
            model_set = true;
        } else if in_whisper && trimmed.starts_with("language = \"") {
            new_lines.push(format!("language = \"{language}\""));
            language_set = true;
        } else {
            new_lines.push(line.to_owned());
        }
    }

    if in_whisper {
        if !model_set {
            new_lines.push(format!("model = \"{model}\""));
        }
        if !language_set {
            new_lines.push(format!("language = \"{language}\""));
        }
    }

    if !new_lines.iter().any(|l| l.trim() == "[whisper]") {
        if !new_lines.is_empty() {
            new_lines.push(String::new());
        }
        new_lines.push("[whisper]".to_owned());
        new_lines.push(format!("model = \"{model}\""));
        new_lines.push(format!("language = \"{language}\""));
    }

    std::fs::write(path, new_lines.join("\n") + "\n")?;
    Ok(())
}

fn patch_voxtype_post_process_hook_at_path(path: &Path) -> AdapterResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut content = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    if content.contains("[output.post_process]") && content.contains("e-voice process") {
        return Ok(());
    }

    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(POST_PROCESS_BLOCK);
    std::fs::write(path, content)?;
    Ok(())
}

// ─── Fake adapter for tests ───────────────────────────────────────────────────

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A configurable [`SystemAdapter`] for unit tests.
    ///
    /// Set `service_running` and `ollama_reachable` to control return values.
    /// Set `write_stt_fails` to `true` to make [`write_voxtype_stt_config`] return
    /// an error.  All mutating calls are recorded in `calls()`.
    pub struct FakeSystemAdapter {
        pub service_running: bool,
        pub ollama_reachable: bool,
        /// When `true`, [`write_voxtype_stt_config`] returns an error.
        pub write_stt_fails: bool,
        /// Voxtype config file content used by `patch_voxtype_post_process_hook`.
        pub voxtype_content: Mutex<String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeSystemAdapter {
        pub fn new(service_running: bool, ollama_reachable: bool) -> Self {
            Self {
                service_running,
                ollama_reachable,
                write_stt_fails: false,
                voxtype_content: Mutex::new(String::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Snapshot of all recorded mutating call names, in order.
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, name: &str) {
            self.calls.lock().unwrap().push(name.to_owned());
        }
    }

    impl SystemAdapter for FakeSystemAdapter {
        fn restart_stt_service(&self) -> AdapterResult<()> {
            self.record("restart_stt_service");
            Ok(())
        }

        fn is_stt_service_running(&self) -> bool {
            self.service_running
        }

        fn reload_service_manager(&self) -> AdapterResult<()> {
            self.record("reload_service_manager");
            Ok(())
        }

        fn enable_autostart(&self, _service_content: &str) -> AdapterResult<()> {
            self.record("enable_autostart");
            Ok(())
        }

        async fn check_ollama_health(&self, _url: &str) -> bool {
            self.ollama_reachable
        }

        fn open_file(&self, path: &Path) -> AdapterResult<()> {
            self.record(&format!("open_file:{}", path.display()));
            Ok(())
        }

        fn send_notification(&self, summary: &str, _body: &str) -> AdapterResult<()> {
            self.record(&format!("send_notification:{summary}"));
            Ok(())
        }

        fn read_voxtype_stt_config(&self) -> AdapterResult<(String, String, String)> {
            Ok(("English".into(), "base.en".into(), "en".into()))
        }

        fn write_voxtype_stt_config(&self, model: &str, language: &str) -> AdapterResult<()> {
            if self.write_stt_fails {
                return Err("write failed (fake)".into());
            }
            self.record(&format!("write_voxtype_stt_config:{model}:{language}"));
            Ok(())
        }

        fn patch_voxtype_post_process_hook(&self) -> AdapterResult<()> {
            let mut content = self.voxtype_content.lock().unwrap();
            if content.contains("[output.post_process]") && content.contains("e-voice process") {
                return Ok(());
            }
            self.record("patch_voxtype_post_process_hook");
            content.push_str(POST_PROCESS_BLOCK);
            Ok(())
        }

        fn voxtype_config_path(&self) -> PathBuf {
            PathBuf::from("/tmp/fake-voxtype-config.toml")
        }

        fn is_binary_available(&self, name: &str) -> bool {
            // Treat "voxtype" and "ollama" as always available in tests.
            matches!(name, "voxtype" | "ollama")
        }

        fn pull_ollama_model(&self, model: &str) -> AdapterResult<()> {
            self.record(&format!("pull_ollama_model:{model}"));
            Ok(())
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fake::FakeSystemAdapter;
    use tempfile::tempdir;

    // ── Fake adapter: patch_voxtype_post_process_hook ────────────────────────

    #[test]
    fn fake_patch_hook_writes_when_absent() {
        let adapter = FakeSystemAdapter::new(true, true);
        adapter.patch_voxtype_post_process_hook().unwrap();
        assert!(adapter.calls().contains(&"patch_voxtype_post_process_hook".to_owned()));
    }

    #[test]
    fn fake_patch_hook_noop_when_already_present() {
        let adapter = FakeSystemAdapter::new(true, true);
        {
            let mut c = adapter.voxtype_content.lock().unwrap();
            *c = "[output.post_process]\ncommand = \"e-voice process\"\ntimeout_ms = 10000\n"
                .to_owned();
        }
        adapter.patch_voxtype_post_process_hook().unwrap();
        assert!(
            !adapter.calls().contains(&"patch_voxtype_post_process_hook".to_owned()),
            "hook should not be re-written when already present"
        );
    }

    // ── LinuxSystemAdapter: patch_voxtype_post_process_hook ──────────────────

    #[test]
    fn linux_patch_hook_noop_when_present() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        let existing = "[output.post_process]\ncommand = \"e-voice process\"\ntimeout_ms = 10000\n";
        std::fs::write(&cfg_path, existing).unwrap();

        patch_voxtype_post_process_hook_at_path(&cfg_path).unwrap();

        let content_after = std::fs::read_to_string(&cfg_path).unwrap();
        assert_eq!(content_after, existing, "hook must be unchanged");
    }

    #[test]
    fn linux_patch_hook_appends_when_absent() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(&cfg_path, "[whisper]\nmodel = \"base.en\"\n").unwrap();

        patch_voxtype_post_process_hook_at_path(&cfg_path).unwrap();

        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(content.contains("[output.post_process]"), "hook should be appended");
        assert!(content.contains("e-voice process"), "hook command should be present");
    }

    // ── LinuxSystemAdapter: write_voxtype_stt_config ─────────────────────────

    #[test]
    fn linux_write_creates_whisper_section() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(&cfg_path, "# empty config\n").unwrap();

        write_voxtype_stt_config_to_path(&cfg_path, "base", "es").unwrap();

        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(content.contains("[whisper]"), "whisper section should exist");
        assert!(content.contains("model = \"base\""), "model should be set");
        assert!(content.contains("language = \"es\""), "language should be set");
    }

    #[test]
    fn linux_write_updates_existing_whisper_section() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        std::fs::write(&cfg_path, "[whisper]\nmodel = \"base.en\"\nlanguage = \"en\"\n").unwrap();

        write_voxtype_stt_config_to_path(&cfg_path, "large", "de").unwrap();

        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(content.contains("model = \"large\""), "model should be updated");
        assert!(content.contains("language = \"de\""), "language should be updated");
        assert!(
            !content.contains("base.en"),
            "old model value should be gone"
        );
    }

    // ── LinuxSystemAdapter: is_binary_available ──────────────────────────────

    #[test]
    fn linux_is_binary_available_finds_known_binary() {
        let adapter = LinuxSystemAdapter;
        assert!(
            adapter.is_binary_available("sh"),
            "sh should be available on any Linux system"
        );
    }

    #[test]
    fn linux_is_binary_available_returns_false_for_gibberish() {
        let adapter = LinuxSystemAdapter;
        assert!(
            !adapter.is_binary_available("zzz-nonexistent-binary-xyz"),
            "gibberish binary should not be found"
        );
    }
}
