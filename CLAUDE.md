# Agent Instructions

Context and instructions for AI agents working on the ears repository.

## Repository Overview

**ears** is a speech recognition daemon for Linux written in Rust. It provides push-to-talk transcription via whisper.cpp with a TUI interface.

### Technology Stack

- **Language**: Rust (2021 edition)
- **Async Runtime**: Tokio
- **TUI**: Ratatui + Crossterm
- **HTTP Client**: Reqwest with rustls
- **Audio**: PipeWire (via `pw-record`)
- **Text Input**: wtype (Hyprland/Omarchy), ydotool (fallback)
- **CLI**: Clap

## Project Structure

```
src/
├── main.rs              # Entry point, command dispatch
├── lib.rs               # Library exports
├── cli.rs               # CLI argument parsing (clap)
├── config.rs            # Configuration management
├── state.rs             # Recording state machine
├── lock.rs              # File locking (single instance)
├── process.rs           # Child process management
├── audio.rs             # Audio device discovery
├── recording.rs         # Recording orchestration
├── whisper.rs           # Whisper.cpp HTTP client
├── desktop.rs           # Notifications, audio feedback, typing
├── streaming.rs         # Streaming transcription
├── streaming_engine.rs  # Streaming engine
├── vad.rs               # Voice activity detection
├── continuous_capture.rs # Continuous capture mode
├── progressive_typing.rs # Progressive text output
└── tui/
    ├── mod.rs           # TUI module exports
    ├── app.rs           # TUI application state
    ├── ui.rs            # TUI rendering
    └── event.rs         # TUI event handling
```

## Development

### Build & Test

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # Run all tests
cargo clippy                   # Lint
cargo fmt                      # Format
```

### Install

```bash
cargo install --path .         # Install to ~/.cargo/bin/ears
```

### Running

```bash
ears                          # Launch TUI (default)
ears toggle                   # Toggle recording (for keybinds)
ears list                     # List audio devices
ears select                   # Interactive device selection
ears server                   # Show/set whisper server
ears --help                   # Full help
```

## Architecture

- **State Machine**: Idle ↔ Recording, file-based state in `$XDG_RUNTIME_DIR/ears/`
- **Single Instance**: File locking prevents concurrent runs
- **Process Isolation**: Audio recording in separate `pw-record` process
- **Async I/O**: Non-blocking HTTP to whisper server

See `docs/ARCHITECTURE.md` for detailed design documentation.

## Code Style

- Use `cargo fmt` before committing
- Run `cargo clippy` and fix warnings
- Use `anyhow` for errors in binaries, `thiserror` for libraries
- Prefer explicit error handling over `.unwrap()`
- Add tests for new functionality

## Commit Guidelines

Use conventional commits:
- `feat:` - New features
- `fix:` - Bug fixes
- `docs:` - Documentation
- `refactor:` - Code refactoring
- `test:` - Test improvements
- `chore:` - Maintenance

## External Dependencies

### Required

| Dependency | Package (Arch) | Purpose |
|------------|----------------|---------|
| `pw-record` | `pipewire` | Audio capture from PipeWire device |
| `pw-cli` | `pipewire` | Audio device discovery and listing |
| `timeout` | `coreutils` | Recording duration limit (wraps pw-record) |
| `pkill` | `procps-ng` | Signal waybar on state changes (SIGRTMIN+9) |
| `notify-send` | `libnotify` | Desktop notifications (info, warning, error) |
| `paplay` | `libpulse` | Audio feedback sounds (start/stop/error beeps) |

### Text Input (one required)

| Dependency | Package (Arch) | Purpose |
|------------|----------------|---------|
| `wtype` | `wtype` | Direct text typing on Hyprland/Wayland (preferred on Omarchy) |
| `ydotool` | `ydotool` | Keyboard simulation fallback (Ctrl+V paste on non-Omarchy) |

On Omarchy/Hyprland systems, `wtype` is used. On other systems, `ydotool` + `wl-copy`/`wl-paste` are used for clipboard-based paste.

### Optional

| Dependency | Package (Arch) | Purpose |
|------------|----------------|---------|
| `hyprctl` | `hyprland` | Keyboard layout detection, Omarchy detection |
| `dconf` | `dconf` | GNOME keyboard layout detection (fallback) |
| `wl-copy` | `wl-clipboard` | Clipboard write (non-Omarchy text input path) |
| `wl-paste` | `wl-clipboard` | Clipboard read/restore (non-Omarchy text input path) |
| `fzf` | `fzf` | Interactive device selection (`ears select`) |
| `column` | `util-linux` | Device list formatting (graceful fallback) |

### External Service

| Dependency | Purpose |
|------------|---------|
| whisper.cpp / faster-whisper server | Transcription API (OpenAI-compatible `/v1/audio/transcriptions` endpoint) |
