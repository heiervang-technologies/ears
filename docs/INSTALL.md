# Installation Guide

## System Requirements

- **Linux** with PipeWire audio (Ubuntu 22.04+, Arch, Fedora 34+, etc.)
- **Rust toolchain** (for building from source)
- A whisper.cpp or OpenAI-compatible ASR server

## Dependencies

### Required

| Dependency | Arch | Ubuntu | Purpose |
|-----------|------|--------|---------|
| `pw-record` | `pipewire` | `pipewire` | Audio capture |
| `pw-cli` | `pipewire` | `pipewire` | Device discovery |
| `timeout` | `coreutils` | `coreutils` | Recording duration limit |

### Text Input (one required)

| Dependency | Arch | Ubuntu | Purpose |
|-----------|------|--------|---------|
| `wtype` | `wtype` | - | Direct Wayland typing (Hyprland, preferred) |
| `ydotool` | `ydotool` | `ydotool` | Keyboard simulation (other systems) |

### Optional

| Dependency | Arch | Ubuntu | Purpose |
|-----------|------|--------|---------|
| `notify-send` | `libnotify` | `libnotify-bin` | Desktop notifications |
| `paplay` | `pulseaudio` | `pulseaudio-utils` | Audio feedback |
| `fzf` | `fzf` | `fzf` | Interactive device selection |
| `wl-copy`/`wl-paste` | `wl-clipboard` | `wl-clipboard` | Clipboard (non-Hyprland text input) |
| `pkill` | `procps-ng` | `procps` | Waybar state refresh |

### Install Commands

```bash
# Arch Linux (Hyprland/Omarchy)
sudo pacman -S pipewire wtype libnotify pulseaudio fzf

# Ubuntu/Debian
sudo apt install pipewire ydotool wl-clipboard libnotify-bin pulseaudio-utils fzf

# Fedora
sudo dnf install pipewire ydotool libnotify pulseaudio-utils fzf
```

## Rust Toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

## Whisper Server Setup

```bash
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
make server                    # CPU
# make server WHISPER_CUDA=1   # GPU

bash ./models/download-ggml-model.sh base.en
./server -m models/ggml-base.en.bin -p 8178
```

Verify: `curl -sf http://localhost:8178/health && echo OK`

## Installing ears

### From Source

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
cargo build --release
cargo install --path .
ears --help
```

### Using install.sh

```bash
./install.sh
# Installs to ~/.local/bin/ears
export PATH="$HOME/.local/bin:$PATH"
```

### From GitHub Releases

```bash
mkdir -p ~/.local/bin
gh release download latest --repo heiervang-technologies/ears --pattern 'ears' --dir ~/.local/bin --clobber
chmod +x ~/.local/bin/ears
```

## Initial Configuration

```bash
ears server http://localhost:8178
ears select   # Choose your microphone
ears          # Launch TUI
```

## ydotool Setup (non-Hyprland only)

```bash
ydotoold &
# Or as systemd service:
systemctl --user enable --now ydotool

# Verify:
ydotool type "test"
```

If ydotool doesn't work, check `/dev/uinput` permissions and ensure you're in the `input` group:
```bash
sudo usermod -aG input $USER   # then log out and back in
```
