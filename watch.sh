#!/usr/bin/env bash
set -euo pipefail
cargo watch -x 'build --release' -s 'systemctl --user stop e-voice.service 2>/dev/null || true; cp target/release/e-voice ~/.local/bin/; systemctl --user start e-voice.service'
