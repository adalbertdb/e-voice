#!/usr/bin/env bash
set -euo pipefail

echo "==> Stopping daemon..."
systemctl --user stop e-voice.service 2>/dev/null || true

echo "==> Building release binary..."
cargo build --release

echo "==> Installing to ~/.local/bin/e-voice..."
cp target/release/e-voice ~/.local/bin/e-voice

echo "==> Starting systemd service..."
systemctl --user start e-voice.service

echo "==> Done."
systemctl --user status --no-pager e-voice.service
