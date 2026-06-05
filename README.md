# ears

A production-grade speech recognition daemon for Linux that integrates with whisper.cpp (or any OpenAI-compatible ASR server) and your desktop workflow.

![ears TUI demo](demo.gif)

## Features

- **Interactive TUI**: Terminal UI with real-time status, VAD mode, live transcription, and configuration (default mode)
- **Push-to-talk**: Bind `ears toggle` to a keyboard shortcut for quick dictation
- **VAD mode**: Voice Activity Detection for hands-free continuous transcription (`ears vad`)
- **Streaming transcription**: Real-time text output using LocalAgreement policy for stable text
- **Text filters**: Optional lowercase conversion and punctuation removal
- **Bash mode**: Constrain dictation to valid shell syntax via grammar-guided decoding — speak commands, get code
- **Language detection**: Automatic language selection from keyboard layout (Hyprland + GNOME)
- **Smart text input**: Uses `wtype` on Hyprland/Wayland, clipboard paste via `ydotool` elsewhere
- **PipeWire audio**: Native support for modern Linux audio stack
- **Audio feedback**: Embedded beep sounds with custom sound override support
- **State management**: File-based locking and state with automatic crash recovery
- **Post-transcribe hooks**: Run custom scripts after each transcription

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

### Bash Mode (dictate shell commands)

Bash mode constrains the speech model's output to valid shell syntax, so spoken
commands land as code (`ls` → `ls`, not `LS`/`Alice`) instead of prose. You say
the command out loud; the grammar keeps it structurally valid bash. It is **not**
translation — say "git status", not "show me the git status".

Enable it per profile in `~/.config/ears/config.<name>.toml`:

```toml
bash_mode = true             # constrain output to the built-in bash grammar
auto_enter = false           # recommended: type the command but DON'T run it
# guided_grammar = "..."     # optional: override the built-in grammar (GBNF)
```

Then use **push-to-talk** — it's the right fit for discrete commands:

```bash
ears -p bash toggle    # speak a command, toggle again → it's typed (not run)
```

Notes:
- Requires a server with grammar-guided decoding. Bash mode routes requests to
  the OpenAI-compatible `/v1/chat/completions` endpoint with
  `structured_outputs.grammar` (e.g. vLLM); the plain transcription endpoint
  does not support it. Normal (non-bash) profiles are unaffected.
- A configured `model` is required in bash mode.
- The command allow-list lives in `grammars/bash.gbnf` — extend it as needed.
- Best with push-to-talk. The streaming/VAD path accumulates text across
  utterances and isn't suited to discrete commands yet.

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
│   ├── recording.rs         # Recording orchestration
│   ├── whisper.rs           # Whisper HTTP client
│   ├── desktop.rs           # Notifications, audio feedback, text input, keyboard detection
│   ├── text_filters.rs      # Text transformation filters
│   ├── streaming.rs         # Streaming transcription + LocalAgreement
│   ├── streaming_engine.rs  # Streaming engine coordinator
│   ├── vad.rs               # Voice activity detection
│   ├── continuous_capture.rs# Continuous audio capture
│   ├── progressive_typing.rs# Progressive text output
│   └── tui/
│       ├── mod.rs           # TUI module exports
│       ├── app.rs           # TUI application state
│       ├── ui.rs            # TUI rendering
│       └── event.rs         # TUI event handling
├── sounds/                  # Embedded sound files
├── docs/                    # Documentation
├── tests/                   # Integration tests
├── install.sh               # Installation script
├── Cargo.toml               # Rust package manifest
├── README.md                # This file
└── CLAUDE.md                # Agent instructions
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
