# User Guide

Installation, configuration, and day-to-day usage of `ears`.

## Installation

### Prerequisites

**Required:**
- Linux with PipeWire audio system
- A whisper.cpp or OpenAI-compatible ASR server running
- `wtype` (Hyprland/Wayland) or `ydotool` (other systems) for text input

**Optional:**
- `notify-send` for desktop notifications
- `paplay` for audio feedback
- `fzf` for interactive device selection

### Installing System Dependencies

#### Arch Linux (Hyprland/Omarchy)
```bash
sudo pacman -S pipewire wtype libnotify pulseaudio fzf
```

#### Ubuntu/Debian
```bash
sudo apt install pipewire ydotool wl-clipboard libnotify-bin pulseaudio-utils fzf
```

#### Fedora
```bash
sudo dnf install pipewire ydotool libnotify pulseaudio-utils fzf
```

### Setting up a Whisper Server

```bash
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
make server                    # CPU only
# make server WHISPER_CUDA=1   # With NVIDIA GPU

bash ./models/download-ggml-model.sh base.en
./server -m models/ggml-base.en.bin -p 8178
```

Models: `tiny.en` (75MB, fastest) → `base.en` (142MB, recommended) → `small.en` (466MB) → `medium.en` (1.5GB) → `large` (2.9GB, best).

### Setting up ydotool (non-Hyprland only)

```bash
ydotoold &
# Or as a systemd service:
systemctl --user enable --now ydotool
```

### Installing ears

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
cargo build --release
cargo install --path .
```

## Quick Start

```bash
# Configure server
ears server http://localhost:8178

# Select microphone
ears select

# Launch TUI
ears
```

## Configuration

### Config Files

`~/.config/ears/` contains:

| File | Purpose |
|------|---------|
| `server` | Whisper server URL |
| `device` | Selected audio device name |
| `language` | Language code (empty = auto-detect from keyboard layout) |
| `text_filters.json` | Text filter settings (JSON) |

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `EARS_SERVER` | Override whisper server URL |
| `EARS_DEVICE` | Override audio device |
| `EARS_LANGUAGE` | Override language code |

### Runtime State

`$XDG_RUNTIME_DIR/ears/` contains:

| File | Purpose |
|------|---------|
| `state` | Current state (idle/recording/transcribing/vad_active) |
| `lock` | Single-instance lock |
| `recording.pid` | PID of recording process |
| `recording.wav` | Temporary audio file |
| `vad.pid` | PID of VAD process |
| `debug.log` | Application logs |

These are automatically cleaned on logout.

### Custom Audio Feedback

Place custom WAV files in `~/.local/share/ears-sounds/`:
- `start.wav` - Recording started
- `done.wav` - Transcription complete
- `bell.wav` - Error

Falls back to embedded sounds if not found.

### Post-Transcribe Hook

Place an executable at `~/.config/ears/hooks/post-transcribe`:
```bash
#!/bin/bash
# $1 = path to audio file copy
# $2 = transcribed text
echo "$2" >> ~/transcriptions.log
```

## Usage

### CLI Commands

```
ears              Launch interactive TUI (default)
ears toggle, t    Toggle recording/transcription
ears vad, v       Toggle headless VAD mode
ears list, l      List audio devices
ears select, s    Select device interactively (fzf)
ears current, c   Show current device
ears server [URL] Show or set whisper server URL
ears help         Show help
```

### Push-to-Talk Workflow

1. Press keyboard shortcut → recording starts (beep)
2. Speak your message
3. Press shortcut again → recording stops, transcription happens
4. Text is typed into your active window

### Keyboard Shortcut Setup

#### Hyprland
```ini
# ~/.config/hypr/bindings.conf
bind = SUPER SHIFT, V, exec, ears toggle
```

#### i3/Sway
```
bindsym $mod+Shift+v exec ears toggle
```

#### GNOME
Settings → Keyboard → Custom Shortcuts → Add `ears toggle`

### Recording Details

- Format: 16kHz, mono, signed 16-bit PCM (WAV)
- 2-minute timeout protection
- Automatic stale process cleanup

### Text Input

On Hyprland/Wayland: uses `wtype` for direct text typing.
On other systems: copies to clipboard via `wl-copy`, then pastes with `ydotool key ctrl+v`. Original clipboard is preserved and restored.

## Troubleshooting

### "Whisper server not running!"
```bash
curl -sf http://localhost:8178/health || echo "Server not responding"
ears server   # Check configured URL
```

### Text not being typed
```bash
# Hyprland: check wtype
which wtype

# Other: check ydotool
pgrep ydotoold || echo "ydotool daemon not running"
ydotoold &
```

### Wrong microphone
```bash
ears list      # See all devices
ears select    # Pick the right one
```

### Check logs
```bash
cat $XDG_RUNTIME_DIR/ears/debug.log
```

## Privacy

- Audio sent to whisper server only (default: localhost)
- No cloud services, no telemetry
- Temporary audio files cleared on logout
- No recordings kept after transcription
