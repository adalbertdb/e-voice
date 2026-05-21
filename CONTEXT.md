# e-voice — Domain Context

## Overview

e-voice is an LLM-powered post-processor for speech-to-text output. A Unix socket daemon
receives raw transcription text, passes it through a local Ollama model with a structured
prompt, and returns cleaned/reformatted text to the caller (e.g. voxtype).

## Key Concepts

### TextProcessor

The core unit of work. Accepts raw transcription text, builds a prompt internally, and
returns processed text from the configured Ollama model. The prompt strategy is an
internal implementation detail — callers do not influence prompt construction.

### Processor Prompt Strategy (internal)

Currently hard-wired to `Mode::Clean`, a universal interpreter prompt that handles
cleaning, formatting, translation, and tone changes based on trailing instructions in
the transcription. This is not exposed in the public interface.

### Profile (future)

`Profile` will replace the current `Mode` enum as a named prompt strategy tied to social
context (e.g. "work", "casual", "technical"). A Profile bundles a prompt template, tone
preferences, and optional post-processing rules. Profiles will be user-configurable and
selectable at runtime without changes to the daemon protocol.

`Mode` in `src/modes.rs` is the current private placeholder that will evolve into the
Profile system. Nothing outside `src/processor.rs` should import `Mode`.

### Daemon Protocol

Newline-delimited JSON over a Unix socket. Requests and responses are tagged enums
(`Request`, `Response` in `src/daemon.rs`).

`Response::Status` reports `{ model, version }` — the active model and daemon version.
Mode/profile information is intentionally absent from the status response until the
Profile system is ready.

### AppState

Holds `override_model` (runtime model override) and `processor`. No mode state.

### e-voice status

Plain output: `active`
JSON/Waybar output: `{ "text": "active", "class": "active", "tooltip": "e-voice active" }`
