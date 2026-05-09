# ears

**Voice-to-text for your Linux desktop.** Press a hotkey, speak, and your words appear wherever the cursor is. Or leave VAD mode on and let it transcribe continuously, hands-free.

Backend-agnostic — works with local [whisper.cpp](https://github.com/ggerganov/whisper.cpp), [faster-whisper](https://github.com/SYSTRAN/faster-whisper), or any OpenAI-compatible ASR endpoint (Groq, OpenAI, your own server).

![ears TUI demo](demo.gif)

## Features

- **Interactive TUI** — real-time status, VAD controls, live transcription, configuration (default mode)
- **Push-to-talk** — bind `ears toggle` to a keyboard shortcut for quick dictation
- **VAD mode** — hands-free voice activity detection with auto-transcription (`ears vad`)
- **Streaming transcription** — real-time text output with LocalAgreement for stable progressive output
- **Volume ducking** — optionally lowers system volume while you're speaking
- **Profiles** — switch between local whisper and cloud APIs (Groq, OpenAI, etc.) per-invocation
- **Language detection** — auto-detects from keyboard layout (Hyprland + GNOME)
- **Smart text input** — `wtype` on Wayland, clipboard paste via `ydotool` elsewhere
- **PipeWire audio** — native support for the modern Linux audio stack
- **Post-transcribe hooks** — run custom scripts after each transcription
- **Audio feedback** — embedded cue sounds, customizable
- **Crash-resilient state** — file-based locking with automatic recovery
- **No telemetry** — audio goes only to the server you configure

## Prerequisites

### Required

- Linux with PipeWire audio system
- A whisper.cpp or OpenAI-compatible ASR server running
- Text input tool: `wtype` (Hyprland/Wayland) or `ydotool` (other systems)

### Optional

- `notify-send` for desktop notifications
- `paplay` for audio feedback
- `fzf` for interactive device selection
- `wl-clipboard` (`wl-copy`/`wl-paste`) for clipboard-based text input on non-Hyprland systems

### Installing Dependencies

```bash
# Arch Linux (Hyprland/Omarchy)
sudo pacman -S pipewire wtype libnotify pulseaudio fzf

# Ubuntu/Debian
sudo apt install pipewire ydotool wl-clipboard libnotify-bin pulseaudio-utils fzf

# Fedora
sudo dnf install pipewire ydotool libnotify pulseaudio-utils fzf
```

### Setting up a Whisper Server

1. Clone and build whisper.cpp:
```bash
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
make server            # CPU only
make server WHISPER_CUDA=1  # With NVIDIA GPU
```

2. Download a model:
```bash
bash ./models/download-ggml-model.sh base.en
```

3. Start the server:
```bash
./server -m models/ggml-base.en.bin -p 8178
```

## Installation

### From GitHub Releases

```bash
mkdir -p ~/.local/bin
gh release download latest --repo heiervang-technologies/ears --pattern 'ears' --dir ~/.local/bin --clobber
chmod +x ~/.local/bin/ears
export PATH="$HOME/.local/bin:$PATH"
```

### From Source

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
cargo build --release
cargo install --path .
```

### Using install.sh

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
./install.sh
```

## Configuration

Configuration is stored in `~/.config/ears/config.toml`:

```toml
server = "http://127.0.0.1:8178"
device = "alsa_input.usb-..."
# language = "en"          # Optional (auto-detects from keyboard layout)
# api_key = "sk-..."       # Optional (for authenticated ASR services)
# model = "whisper-large-v3-turbo"  # Optional (for cloud APIs that require it)

[text_filters]
lowercase = false
remove_punctuation = false
```

### Profiles

Named profiles let you switch between ASR backends. Create `config.{name}.toml` alongside the default:

```bash
# ~/.config/ears/config.toml        ← default (e.g. local whisper)
# ~/.config/ears/config.groq.toml   ← Groq cloud API

ears -p groq          # Launch TUI with Groq profile
ears -p groq toggle   # Push-to-talk with Groq profile
```

You can also set the profile via environment variable:

```bash
export EARS_PROFILE=groq
ears toggle
```

Priority: `-p` flag > `EARS_PROFILE` env var > default `config.toml`.

### Set server URL

```bash
ears server http://localhost:8178   # Set
ears server                          # Show current
```

### Select microphone

```bash
ears list      # List available devices
ears select    # Interactive selection (fzf)
ears current   # Show current device
```

### Environment variables

Environment variables override config file values:

| Variable | Purpose |
|----------|---------|
| `EARS_SERVER` | Override whisper server URL |
| `EARS_DEVICE` | Override audio device |
| `EARS_LANGUAGE` | Override language code |
| `EARS_API_KEY` | Override API key |
| `EARS_MODEL` | Override model name |
| `EARS_PROFILE` | Set config profile |

## Usage

### TUI Mode (default)

```bash
ears
```

Launches an interactive terminal UI with status monitoring, VAD mode controls, configuration, and logs.

### Push-to-Talk (keyboard shortcut)

Bind `ears toggle` to a keyboard shortcut:

```ini
# Hyprland (~/.config/hypr/bindings.conf)
bind = SUPER SHIFT, V, exec, ears toggle

# i3/Sway
bindsym $mod+Shift+v exec ears toggle
```

Then: press shortcut → speak → press again → text is typed.

### VAD Mode (headless)

```bash
ears vad    # Start VAD (or stop if already running)
```

Continuously listens and auto-transcribes when speech is detected. Toggle on/off by running the command again.

### All Commands

```
ears                   Launch interactive TUI (default)
ears -p groq           Launch TUI with named profile
ears toggle, t         Toggle recording/transcription
ears vad, v            Toggle VAD mode
ears list, l           List audio devices
ears select, s         Select device interactively
ears current, c        Show current device
ears server [URL]      Show or set whisper server URL
ears help              Show help
```

## How It Works

### State Machine

States: `Idle` → `Recording` → `Transcribing` → `Idle`, plus `VadActive` for VAD mode.

State is persisted to `$XDG_RUNTIME_DIR/ears/state` and reconciled on startup.

### Transcription Flow

1. Stops `pw-record` process (SIGTERM)
2. Waits 300ms for file flush
3. Validates WAV file (RIFF header check)
4. Detects language from keyboard layout (if not configured)
5. POSTs audio to `/v1/audio/transcriptions` endpoint
6. Filters silence artifacts ("Thank you.", etc.)
7. Applies text filters (lowercase, punctuation removal)
8. Types text via `wtype` or clipboard paste
9. Runs post-transcribe hook if configured
10. Cleans up temporary files

### Post-Transcribe Hook

Place an executable script at `~/.config/ears/hooks/post-transcribe`. It receives:
- `$1` - Path to a copy of the audio file
- `$2` - The transcribed text

## Custom Sounds

Place custom WAV files in `~/.local/share/ears-sounds/`:
- `start.wav` - Recording started
- `done.wav` - Transcription complete
- `bell.wav` - Error occurred

Falls back to embedded sounds if not found.

## Troubleshooting

### "Whisper server not running!"
- Check server: `curl http://localhost:8178/health` (local) or `curl -H "Authorization: Bearer $KEY" https://api.groq.com/openai/v1/models` (cloud)
- Check config: `ears server`

### "No active recording"
- Recording may have timed out (2 minute limit)
- Check state: `cat $XDG_RUNTIME_DIR/ears/state`
- Check logs: `cat $XDG_RUNTIME_DIR/ears/debug.log`

### Text isn't being typed
- Hyprland: ensure `wtype` is installed
- Other: ensure `ydotoold` is running (`pgrep ydotoold`)
- Test manually: `wtype "test"` or `ydotool type "test"`

### Wrong microphone
```bash
ears list       # See all devices
ears select     # Pick the right one
ears current    # Verify
```

## Development

### Project Structure

```
ears/
├── src/
│   ├── main.rs              # Entry point, command dispatch
│   ├── lib.rs               # Library exports
│   ├── cli.rs               # CLI argument parsing (clap)
│   ├── config.rs            # Configuration management
│   ├── state.rs             # Recording state machine
│   ├── lock.rs              # File locking (single instance)
│   ├── process.rs           # Child process management
│   ├── audio.rs             # Audio device discovery
│   ├── whisper.rs           # Whisper HTTP client
│   ├── desktop.rs           # Notifications, feedback, text input, keyboard detection
│   ├── ducker.rs            # System volume ducking during speech
│   ├── text_filters.rs      # Text transformation filters
│   ├── streaming.rs         # Streaming transcription + LocalAgreement
│   ├── streaming_engine.rs  # Streaming engine coordinator
│   ├── vad.rs               # Voice activity detection (Silero)
│   ├── continuous_capture.rs# Continuous audio capture
│   ├── progressive_typing.rs# Progressive text output
│   ├── ipc.rs               # Unix domain socket IPC server
│   ├── ws_input.rs          # WebSocket audio input mode
│   └── tui/
│       ├── mod.rs           # TUI module exports
│       ├── app.rs           # TUI application state
│       ├── ui.rs            # TUI rendering
│       └── event.rs         # TUI event handling
├── sounds/                  # Embedded cue sounds
├── docs/                    # Architecture and design docs
├── tests/                   # Integration and snapshot tests
├── install.sh               # Installation script
├── Cargo.toml               # Rust package manifest
└── README.md                # This file
```

### Build & Test

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # Run all tests
cargo clippy                   # Lint
cargo fmt                      # Format
RUST_LOG=debug cargo run       # Run with debug logging
```

## Security

- Audio is sent to the configured whisper server only (defaults to localhost)
- API keys are stored in `config.toml` (ensure appropriate file permissions)
- Temporary audio files in `$XDG_RUNTIME_DIR` (cleared on logout)
- No audio is saved permanently
- No telemetry

## License

MIT

## Credits

Built with:
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) - Fast whisper inference
- [PipeWire](https://pipewire.org/) - Modern Linux audio
- [ratatui](https://github.com/ratatui-org/ratatui) - TUI framework
- [wtype](https://github.com/atx/wtype) / [ydotool](https://github.com/ReimuNotMoe/ydotool) - Text input automation
