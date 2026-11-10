<div align="center">

# 🎙️ e-voice

**Local-first dictation post-processor for Linux. Speak, and a local LLM turns your raw transcription into clean, ready-to-paste text.**

![Rust](https://img.shields.io/badge/Rust-2024-000000?logo=rust&logoColor=white)
![Tokio](https://img.shields.io/badge/Async-Tokio-463B5C)
![Axum](https://img.shields.io/badge/HTTP-Axum-6E4A7E)
![Tauri](https://img.shields.io/badge/App-Tauri%20v2-24C8DB?logo=tauri&logoColor=white)
![Ollama](https://img.shields.io/badge/LLM-Ollama%20%7C%20LM%20Studio-000000?logo=ollama&logoColor=white)
![Linux](https://img.shields.io/badge/Platform-Linux%20(Hyprland)-FCC624?logo=linux&logoColor=black)

</div>

---

## ✨ What it does

Speech-to-text tools give you exactly what you said: *"so um I think we should, like, move the meeting to thursday, no wait friday"*.
**e-voice** sits between the transcriber ([voxtype](https://github.com/peteonrails/voxtype)) and your cursor, and sends that text through a **local LLM** to fix it before it gets typed:

> *"I think we should move the meeting to Friday."*

Everything runs on your machine: no cloud APIs, no audio or text leaves your computer.

```
 hold F9 ──► voxtype (speech → raw text) ──► e-voice (raw → polished) ──► typed at cursor
                                                  │
                                                  └─► Ollama / LM Studio (local model)
```

## 🧠 Highlights

| | |
|---|---|
| **Async Rust service** | Built on Tokio with an **Axum** HTTP API (`/health`, `/status`, `/process`) bound to `127.0.0.1`. |
| **Swappable LLM backends** | An `LlmBackend` trait with **Ollama** and **LM Studio** adapters: load/unload models, list models, health checks, configurable `keep_alive`. |
| **Profile system** | Named prompt strategies (`universal_interpreter`, `formal`, `casual`, `bullet`, `translate:<lang>`) that persist across restarts. The default profile also understands spoken inline commands such as *"make it a list:"* or *"translate to French:"*. |
| **Desktop integration** | **Tauri v2** app shell with system tray, `--headless` mode, Waybar status module, Hyprland keybinding snippet and a systemd user service. |
| **Tooling & DX** | `setup` wizard that wires voxtype, pulls models and generates config snippets; `doctor` command that diagnoses the whole pipeline. Arch `PKGBUILD` included. |
| **Clean architecture & tests** | OS calls isolated behind a `SystemAdapter` seam, a single transport module, structured diagnostics. **60+ unit and integration tests**, including an HTTP server tested against a mocked Ollama. |
| **Issue-driven workflow** | Every feature starts as a GitHub issue (with a PRD for the desktop app) and lands through a PR. |

## 🧩 Profiles

| Profile | What it does |
|---|---|
| `universal_interpreter` *(default)* | Removes filler words, fixes punctuation, keeps your meaning, and follows inline spoken instructions. |
| `formal` | Rewrites in a professional tone. |
| `casual` | Rewrites in a relaxed, friendly tone. |
| `bullet` | Formats your ideas as Markdown bullet points. |
| `translate:<lang>` | Translates to the target language, e.g. `translate:es`. |

## 🚀 Getting started

### Prerequisites

- Linux desktop session (developed on Omarchy / Hyprland)
- [voxtype](https://github.com/peteonrails/voxtype) installed and working
- [Ollama](https://ollama.com/) or [LM Studio](https://lmstudio.ai/) running locally
- Rust toolchain (edition 2024)

### Install

```bash
cargo install --path .
```

Or, on Arch Linux, with the bundled package:

```bash
cd packaging && makepkg -si
```

### First-time setup

```bash
e-voice setup
```

The wizard checks that voxtype and Ollama are available, hooks e-voice into `~/.config/voxtype/config.toml`, pulls the default models, generates Hyprland and Waybar snippets in `~/.config/e-voice/`, and enables the systemd user service if available.

### Run

```bash
e-voice --headless   # HTTP server only, on 127.0.0.1:39539
e-voice tray         # daemon + system tray icon
e-voice doctor       # check config, daemon, backend and models
```

Default Hyprland mapping: **hold `F9`** to record, **release** to transcribe, clean up and type.

## ⚙️ Configuration

`~/.config/e-voice/config.toml`

```toml
[llm]
backend = "ollama"              # or "lm_studio"
url = "http://localhost:11434"
model = "llama3.2:1b"
keep_alive_secs = 300           # -1 keeps the model loaded forever
```

The active profile is stored in `~/.config/e-voice/state.toml` and restored on restart.

### Waybar

```json
"custom/e-voice": {
  "exec": "e-voice status --follow --format json",
  "return-type": "json",
  "format": "{}",
  "tooltip": true
}
```

## 🗺️ Roadmap

e-voice is evolving from a Linux CLI into a **cross-platform desktop app with swappable speech-to-text and LLM backends** ([PRD](https://github.com/adalbertdb/e-voice/issues/9)).

- [x] HTTP process endpoint ([#10](https://github.com/adalbertdb/e-voice/issues/10))
- [x] Tauri app shell ([#11](https://github.com/adalbertdb/e-voice/issues/11))
- [x] Profile system with built-in presets ([#12](https://github.com/adalbertdb/e-voice/issues/12))
- [x] LLM backend trait + LM Studio adapter ([#14](https://github.com/adalbertdb/e-voice/issues/14))
- [ ] Cross-platform typing ([#13](https://github.com/adalbertdb/e-voice/issues/13))
- [ ] Backend settings UI ([#15](https://github.com/adalbertdb/e-voice/issues/15))
- [ ] Speech-to-text discovery & setup guide ([#16](https://github.com/adalbertdb/e-voice/issues/16))
- [ ] Dashboard and custom profiles ([#17](https://github.com/adalbertdb/e-voice/issues/17), [#18](https://github.com/adalbertdb/e-voice/issues/18))

## 📁 Project structure

```
src/
├── main.rs            # CLI (clap): --headless, tray, process, status, setup, doctor
├── http_server.rs     # Axum API: /health, /status, /process
├── llm_backend.rs     # LlmBackend trait + Ollama and LM Studio adapters
├── modes.rs           # Profile system and prompt templates
├── processor.rs       # TextProcessor: raw text + profile → polished text
├── daemon.rs          # Unix socket daemon (legacy, being replaced by HTTP)
├── transport.rs       # Unix socket transport shared by CLI and tray
├── diagnostics.rs     # `doctor` checks
├── setup.rs           # First-run wizard
├── system_adapter.rs  # Seam between business logic and the host OS
└── tray.rs            # System tray (ksni)
src-tauri/             # Tauri v2 desktop shell
src-frontend/          # Vite + TypeScript UI
packaging/             # PKGBUILD + systemd user service
```

## 👤 Author

**Dennis Adalbert**: [GitHub](https://github.com/adalbertdb) · [LinkedIn](https://www.linkedin.com/in/dennis-adalbert-boghean-5413582b8/) · [adalbertdb.com](https://adalbertdb.com)
