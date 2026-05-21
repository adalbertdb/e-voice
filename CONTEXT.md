# e-voice — Domain Context

## Overview

e-voice is an LLM-powered post-processor for speech-to-text output. A Unix socket daemon
receives raw transcription text, passes it through a local Ollama model with a structured
prompt, and returns cleaned/reformatted text to the caller (e.g. voxtype).

## Key Concepts

### TextProcessor

The core unit of work. Accepts raw transcription text and a `Profile`, builds a prompt
internally via `Profile::prompt_for`, and returns processed text from the configured
Ollama model. The prompt strategy is determined entirely by the active `Profile`.

### Profile

`Profile` (`src/modes.rs`) is the named prompt strategy that controls how transcribed
speech is rewritten.  It replaces the old `Mode::Clean` placeholder.

Built-in variants:
- `UniversalInterpreter` — default; cleans speech and handles inline instructions
- `Formal` — rewrites in professional tone
- `Casual` — rewrites in relaxed, friendly tone
- `Bullet` — formats as Markdown bullet points
- `Translate(lang)` — translates to the given language code (e.g. `"es"`, `"fr"`)

`Profile::prompt_for(text: &str) -> String` builds the full LLM prompt by substituting
the input text into the profile's instruction template.

Profiles serialise to a compact string (`"universal_interpreter"`, `"formal"`,
`"translate:es"`, etc.) for HTTP payloads and TOML state files.

### Profile Persistence

The active profile is stored in `AppState` and persisted to `state.toml` (alongside
`config.toml` in the e-voice config directory) whenever it changes.  On daemon restart
the last active profile is restored via `PersistentState::load()`.

Custom profiles are not yet implemented; they are planned for a future issue.

### Daemon Protocol

Newline-delimited JSON over a Unix socket. Requests and responses are tagged enums
(`Request`, `Response` in `src/daemon.rs`).

`Response::Status` reports `{ model, version, profile }` — the active model, daemon
version, and active profile name.

`Request::Process` accepts an optional `profile` field.  When present the supplied
profile is used for that request **and** becomes the new active profile (persisted to
`state.toml`).  When absent the current `AppState.active_profile` is used.

### AppState

Holds `override_model` (runtime model override), `active_profile` (current `Profile`),
and `processor`.  The active profile defaults to `UniversalInterpreter` on a fresh start
and is restored from `state.toml` on subsequent starts.

### e-voice status

Plain output: `active`
JSON/Waybar output: `{ "text": "active", "class": "active", "tooltip": "e-voice active" }`
