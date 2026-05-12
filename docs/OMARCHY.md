# Omarchy Integration Guide

This guide covers Omarchy-specific setup for `e-voice` on Hyprland + Waybar + Walker.

## Install

Use one of:

- AUR/local package workflow (`packaging/PKGBUILD`)
- Cargo install (`cargo install --path .`)

Then run:

```bash
e-voice setup
```

## Hyprland keybindings

After setup, copy snippet from:

`~/.config/e-voice/hyprland-snippet.conf`

Into your Hyprland bindings file (usually `~/.config/hypr/bindings.conf`).

Default bindings:

```conf
bind = , F9, exec, voxtype record start
bindr = , F9, exec, voxtype record stop
bind = SUPER SHIFT, D, exec, e-voice menu | walker --dmenu
```

Reload Hyprland config after editing.

## Waybar module

After setup, copy snippet from:

`~/.config/e-voice/waybar-snippet.json`

Into your Waybar config JSON/JSONC under modules.

The module reads daemon mode updates from:

```bash
e-voice status --follow --format json
```

For Omarchy desktop usage, run the graphical mode with:

```bash
e-voice tray
```

For debugging or service mode, use the headless daemon instead:

```bash
e-voice daemon
```

## Walker integration

Walker mode picker is powered by:

```bash
e-voice menu | walker --dmenu
```

This returns a JSON list where each option executes `e-voice mode <mode>`.

## Changing modes

Available options:

- Tray icon (left-click cycle, right-click explicit selection)
- CLI (`e-voice mode formal`)
- Walker picker (`SUPER+SHIFT+D` from snippet)

To validate current mode:

```bash
e-voice status
```
