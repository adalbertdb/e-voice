# e-voice

`e-voice` is a local-first Linux companion for voice dictation workflows. It receives raw text from `voxtype`, applies mode-specific LLM post-processing through Ollama, and returns polished text ready to type at cursor.

## Prerequisites

- [`voxtype`](https://github.com/your-org/voxtype) installed and working
- [`ollama`](https://ollama.com/) installed and running
- Linux desktop session (Omarchy/Hyprland focused)

## Install

### Cargo

```bash
cargo install --path .
```

### Arch package (local PKGBUILD)

```bash
makepkg -si
```

## First-time setup

Run the setup wizard once:

```bash
e-voice setup
```

The wizard:
- validates `voxtype` and `ollama`
- patches `~/.config/voxtype/config.toml` with `e-voice process`
- pulls default models (`llama3.2:1b`, `qwen2.5:1.5b`)
- generates Hyprland and Waybar snippets in `~/.config/e-voice/`
- enables `e-voice` user service if available

## Usage

### Start daemon

```bash
e-voice daemon
```

This starts the headless Unix socket daemon used by `e-voice process` and `e-voice status`.

### Start tray mode

```bash
e-voice tray
```

This starts the daemon plus the desktop tray UI. Use this only in a graphical session.

### Dictation hotkeys (Hyprland)

Use generated snippet (`~/.config/e-voice/hyprland-snippet.conf`) with default mapping:

- Hold `F9` → `voxtype record start`
- Release `F9` → `voxtype record stop`

### Change mode

- Tray: left-click cycles modes, right-click opens mode menu
- CLI: `e-voice mode <mode>`
- Walker: `e-voice menu | walker --dmenu`

### Status for Waybar / scripts

```bash
e-voice status --format json
e-voice status --follow --format json
```

### Diagnose pipeline health

Run:

```bash
e-voice doctor
```

This checks:
- config load and active Ollama URL
- daemon socket presence and daemon status reachability
- Ollama `/api/tags` reachability
- whether configured models exist in local Ollama

## Available modes

| Mode | Description |
|------|-------------|
| `clean` | Remove filler words, fix punctuation, preserve meaning |
| `formal` | Rewrite in professional tone |
| `casual` | Rewrite in relaxed tone |
| `bullet` | Format ideas as bullet points |
| `translate:<lang>` | Translate to target language (e.g. `translate:es`) |

## Configuration

Config file: `~/.config/e-voice/config.toml`

Key sections:

- `[ollama]` base URL and fallback model
- `[models]` per-mode model mapping
- `[mode]` default mode at startup

State file: `~/.config/e-voice/state.toml` stores active mode between restarts.

## Waybar integration

Add generated snippet from `~/.config/e-voice/waybar-snippet.json` to your Waybar `custom/*` modules.

Expected runtime command:

```json
"custom/e-voice": {
  "exec": "e-voice status --follow --format json",
  "return-type": "json",
  "format": "{}",
  "tooltip": true
}
```
