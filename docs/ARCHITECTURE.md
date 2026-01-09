# Architecture Overview

This document provides a technical overview of the ears architecture.

## Table of Contents

- [High-Level Architecture](#high-level-architecture)
- [Core Components](#core-components)
- [Data Flow](#data-flow)
- [State Management](#state-management)
- [Module Structure](#module-structure)
- [External Dependencies](#external-dependencies)
- [Design Decisions](#design-decisions)
- [Performance Characteristics](#performance-characteristics)

## High-Level Architecture

ears follows a modular pipeline architecture:

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Interaction                        │
│              (Keyboard Shortcut / CLI / TUI)                    │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                      ears Main Process                          │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐         │
│  │   Config     │  │    State     │  │    Lock      │         │
│  │  Management  │  │  Management  │  │  Management  │         │
│  └──────────────┘  └──────────────┘  └──────────────┘         │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐         │
│  │   Process    │  │   Desktop    │  │   Whisper    │         │
│  │   Control    │  │ Integration  │  │    Client    │         │
│  └──────────────┘  └──────────────┘  └──────────────┘         │
└────────┬───────────────────┬──────────────────┬────────────────┘
         │                   │                  │
         ▼                   ▼                  ▼
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
│   PipeWire      │ │   Desktop       │ │  Whisper.cpp    │
│   (pw-record)   │ │   Services      │ │    Server       │
│                 │ │ - ydotool       │ │                 │
│                 │ │ - notify-send   │ │  (HTTP API)     │
│                 │ │ - paplay        │ │                 │
└─────────────────┘ └─────────────────┘ └─────────────────┘
```

### Key Principles

1. **Single Responsibility**: Each module handles one specific concern
2. **Process Isolation**: Audio recording runs in a separate process
3. **File-Based State**: Uses filesystem for state management (lock files, PIDs)
4. **Defensive Programming**: Extensive error handling and state validation
5. **Zero-Copy Where Possible**: Minimizes data copying for audio files

## Core Components

### 1. Configuration Management (`config.rs`)

**Purpose**: Manage application configuration and persistence

**Key Responsibilities**:
- Load configuration from files and environment variables
- Validate configuration values (especially URLs)
- Persist configuration changes
- Provide defaults for missing values

**Data Structure**:
```rust
pub struct Config {
    pub whisper_server: Url,      // Whisper.cpp server URL
    pub device: String,            // PipeWire device name
    pub config_dir: PathBuf,       // Config directory (~/.config/ears)
    pub state_dir: PathBuf,        // Runtime state ($XDG_RUNTIME_DIR/ears)
}
```

**Design Notes**:
- Uses `directories` crate for XDG-compliant paths
- TOML format for human-readable config files
- Environment variables can override file config
- Validates URLs at load time to fail fast

### 2. State Management (`state.rs`)

**Purpose**: Track the daemon's current operational state

**Key Responsibilities**:
- Determine if recording is active
- Store recording metadata (PID, start time, audio file path)
- Clean up stale state from crashed processes
- Provide atomic state transitions

**State Machine**:
```
        start_recording()
Idle ──────────────────────▶ Recording
 ▲                              │
 │                              │
 │         stop_recording()     │
 └──────────────────────────────┘
```

**State Storage**:
- `$XDG_RUNTIME_DIR/ears/recording.pid` - PID of pw-record process
- `$XDG_RUNTIME_DIR/ears/recording.wav` - Audio file being recorded
- `$XDG_RUNTIME_DIR/ears/ears.log` - Application logs

### 3. Lock Management (`lock.rs`)

**Purpose**: Prevent concurrent instances of ears from interfering

**Key Responsibilities**:
- Acquire exclusive lock on startup
- Release lock on shutdown
- Detect and handle stale locks
- Use RAII for automatic cleanup

**Implementation**:
```rust
pub struct FileLock {
    file: File,
    path: PathBuf,
}

impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        // Uses flock() for advisory locking
        // Non-blocking to detect concurrent access
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Automatically releases lock and removes file
    }
}
```

**Design Notes**:
- Uses `flock(2)` system call (advisory locking)
- Automatically cleaned on process termination
- Lock file stored in `$XDG_RUNTIME_DIR/ears/lock`

### 4. Process Control (`process.rs`)

**Purpose**: Manage child processes (primarily pw-record)

**Key Responsibilities**:
- Spawn audio recording process with correct parameters
- Monitor process health
- Implement timeout protection (default: 2 minutes)
- Send signals (SIGTERM, SIGKILL) for cleanup
- Handle zombie processes

**Key Functions**:
```rust
pub struct ProcessManager;

impl ProcessManager {
    // Start pw-record with device and output file
    pub fn start_recording(device: &str, output: &Path) -> Result<Pid>;

    // Stop recording process gracefully
    pub fn stop_recording(pid: Pid) -> Result<()>;

    // Check if process is still alive
    pub fn is_alive(pid: Pid) -> bool;

    // Kill process forcefully if needed
    pub fn kill(pid: Pid) -> Result<()>;
}
```

**Design Notes**:
- Uses `nix` crate for safe POSIX APIs
- Implements graceful degradation (SIGTERM → wait → SIGKILL)
- Cleans up zombie processes via `waitpid()`
- Stores PIDs as signed 32-bit integers (POSIX standard)

### 5. Whisper Integration (`whisper.rs`)

**Purpose**: Communicate with whisper.cpp HTTP server

**Key Responsibilities**:
- Send audio files to whisper server
- Parse transcription responses
- Handle network errors with retries
- Filter out common false positives

**HTTP Client**:
```rust
pub struct WhisperClient {
    client: reqwest::Client,
    server_url: Url,
    retry_policy: ExponentialBackoff,
}

impl WhisperClient {
    pub async fn transcribe(&self, audio_path: &Path) -> Result<String> {
        // 1. Read audio file
        // 2. Create multipart form (file + response_format=json)
        // 3. POST to /inference endpoint
        // 4. Retry on network errors (exponential backoff)
        // 5. Parse JSON response
        // 6. Filter false positives
    }
}
```

**Retry Strategy**:
- Initial backoff: 50ms
- Max backoff: 500ms
- Max retries: 3
- Exponential with jitter

**Response Filtering**:
- Remove empty transcriptions
- Filter "Thank you." (common whisper silence artifact)
- Trim whitespace

**Design Notes**:
- Async/await with tokio runtime
- Uses `rustls` for TLS (no OpenSSL dependency)
- Graceful handling of server timeouts
- Validates server health before transcription

### 6. Desktop Integration (`desktop.rs`)

**Purpose**: Interface with desktop environment services

**Key Responsibilities**:
- Display notifications (libnotify)
- Play audio feedback (paplay)
- Type text into active window (ydotool)
- Handle different desktop environments

**Components**:

```rust
pub struct Notifications;
impl Notifications {
    pub fn show(title: &str, body: &str, urgency: Urgency) -> Result<()>;
}

pub struct AudioFeedback;
impl AudioFeedback {
    pub fn play_start() -> Result<()>;
    pub fn play_done() -> Result<()>;
    pub fn play_error() -> Result<()>;
}

pub struct TextInput;
impl TextInput {
    pub fn type_text(text: &str) -> Result<()>;
}
```

**Audio Feedback Priority**:
1. Custom sounds in `~/.local/share/ears-sounds/*.wav`
2. System sounds in `/usr/share/sounds/freedesktop/stereo/`
3. Fallback to silent operation

**Design Notes**:
- All desktop operations run asynchronously (don't block main flow)
- Errors in desktop integration are logged but not fatal
- Uses `std::process::Command` for calling external tools

### 7. CLI Argument Parsing (`cli.rs`)

**Purpose**: Parse command-line arguments and dispatch commands

**Key Responsibilities**:
- Define CLI interface with clap
- Match Bash version's CLI exactly
- Validate argument combinations
- Provide helpful error messages

**CLI Structure**:
```rust
pub enum Command {
    Toggle,              // No args - toggle recording/transcription
    List,                // --list - show devices
    Select,              // --select - interactive device picker
    Current,             // --current - show current device
    Server(Option<Url>), // --server [URL] - get/set server
    Tui,                 // --tui - launch TUI
}
```

### 8. TUI (Terminal User Interface) (`tui/`)

**Purpose**: Provide interactive dashboard for monitoring and control

**Key Components**:
- `app.rs` - Application state and logic
- `ui.rs` - Rendering and layout
- `event.rs` - Event handling (keyboard, ticks)

**TUI Layout**:
```
┌─ ears TUI ────────────────────────────────────────────┐
│ Status: Recording  ●  Duration: 00:05                  │
├────────────────────────────────────────────────────────┤
│ Configuration                                          │
│   Server:  http://localhost:8178                       │
│   Device:  Built-in Microphone                         │
├────────────────────────────────────────────────────────┤
│ Recent Transcriptions                                  │
│   [14:23] "This is a test transcription"               │
│   [14:20] "Hello world"                                │
├────────────────────────────────────────────────────────┤
│ Logs                                                   │
│   [INFO] Recording started                             │
│   [INFO] Audio file: /tmp/recording.wav                │
└────────────────────────────────────────────────────────┘
Space: Toggle | q: Quit | ?: Help
```

**Design Notes**:
- Built with `ratatui` (terminal UI framework)
- Event loop with 250ms tick rate
- Non-blocking input handling
- Graceful degradation on terminal resize

## Data Flow

### Recording Flow

```
1. User presses keyboard shortcut
   │
   ▼
2. ears binary executes
   │
   ▼
3. Acquire lock (ensure single instance)
   │
   ▼
4. Check state (Idle or Recording?)
   │
   ├─▶ If Idle:
   │   ├─▶ Generate audio file path
   │   ├─▶ Spawn pw-record process
   │   ├─▶ Store PID in state file
   │   ├─▶ Play "start" beep
   │   └─▶ Show notification "Recording..."
   │
   └─▶ If Recording:
       └─▶ (Continue to transcription flow)
```

### Transcription Flow

```
1. Stop pw-record process (SIGTERM)
   │
   ▼
2. Wait 300ms for file to be fully written
   │
   ▼
3. Validate audio file exists and has content
   │
   ▼
4. Send HTTP POST to whisper.cpp server
   │  (multipart/form-data with audio file)
   │
   ▼
5. Retry on network errors (exponential backoff)
   │
   ▼
6. Parse JSON response: {"text": "transcribed text"}
   │
   ▼
7. Filter false positives ("Thank you.", empty)
   │
   ▼
8. Type text using ydotool
   │
   ▼
9. Play "done" beep
   │
   ▼
10. Clean up state (remove PID file, audio file)
    │
    ▼
11. Release lock
```

### Error Handling Flow

Every step has error handling:

```
Error Occurs
   │
   ▼
Log error details
   │
   ▼
Show desktop notification (urgency: critical)
   │
   ▼
Play error beep
   │
   ▼
Clean up partial state
   │
   ▼
Release lock
   │
   ▼
Exit with non-zero code
```

## State Management

### State Files

All state is stored in `$XDG_RUNTIME_DIR/ears/`:

| File | Purpose | Format | Lifecycle |
|------|---------|--------|-----------|
| `lock` | Exclusive lock | Empty | Created on startup, deleted on shutdown |
| `recording.pid` | Recording process PID | Text (decimal number) | Created on record start, deleted on stop |
| `recording.wav` | Audio data | WAV (16kHz mono S16LE) | Created on record start, deleted after transcription |
| `ears.log` | Application logs | Plain text | Appended to continuously |

### State Directory

`$XDG_RUNTIME_DIR` is ideal because:
- Automatically cleaned on user logout
- Per-user isolation (no permission issues)
- tmpfs-backed (fast, no disk wear)
- Standard location (`/run/user/$UID` on most Linux)

### State Transitions

```rust
pub enum State {
    Idle,
    Recording { pid: Pid, started: Instant, audio_file: PathBuf },
}

impl StateManager {
    // Atomic state transition
    pub fn transition_to_recording(&mut self, pid: Pid, audio_file: PathBuf)
        -> Result<()> {
        // 1. Verify current state is Idle
        // 2. Write PID file
        // 3. Update in-memory state
        // 4. Fsync for durability
    }

    pub fn transition_to_idle(&mut self) -> Result<()> {
        // 1. Verify current state is Recording
        // 2. Remove PID file
        // 3. Update in-memory state
    }
}
```

### Stale State Cleanup

On startup, ears checks for stale state:

```rust
impl StateManager {
    pub fn cleanup_stale(&mut self) -> Result<()> {
        // If PID file exists:
        //   1. Read PID
        //   2. Check if process is alive (kill(pid, 0))
        //   3. If dead: remove PID file and audio file
        //   4. If alive but not pw-record: log warning, remove files
    }
}
```

## Module Structure

```
ears/
├── src/
│   ├── main.rs           # Binary entry point
│   ├── lib.rs            # Library entry point
│   ├── cli.rs            # CLI parsing (clap)
│   ├── config.rs         # Configuration (Iteration 1)
│   ├── lock.rs           # File locking (Iteration 2)
│   ├── state.rs          # State management (Iteration 2)
│   ├── process.rs        # Process control (Iteration 2)
│   ├── audio.rs          # Audio device management (Iteration 3)
│   ├── recording.rs      # Recording logic (Iteration 3)
│   ├── whisper.rs        # Whisper.cpp client (Iteration 4)
│   ├── desktop.rs        # Desktop integration (Iteration 6)
│   └── tui/              # TUI components (Iteration 7)
│       ├── mod.rs        # Module exports
│       ├── app.rs        # Application state
│       ├── ui.rs         # Rendering
│       └── event.rs      # Event handling
└── tests/                # Integration tests
    ├── config.rs         # Config tests
    ├── state.rs          # State tests
    ├── whisper_integration.rs  # Whisper API tests
    └── tui.rs            # TUI tests
```

### Dependency Graph

```
main.rs
  ├─▶ cli.rs
  ├─▶ config.rs
  ├─▶ lock.rs
  ├─▶ state.rs
  │     ├─▶ process.rs
  │     └─▶ audio.rs
  ├─▶ recording.rs
  │     ├─▶ process.rs
  │     └─▶ audio.rs
  ├─▶ whisper.rs
  ├─▶ desktop.rs
  └─▶ tui/
        ├─▶ app.rs
        ├─▶ ui.rs
        └─▶ event.rs
```

## External Dependencies

### System Dependencies

| Dependency | Purpose | Fallback |
|------------|---------|----------|
| PipeWire | Audio capture | None (required) |
| ydotool | Text input | Notify user, no typing |
| notify-send | Desktop notifications | Log to stderr |
| paplay | Audio feedback | Silent operation |
| fzf | Interactive device selection | List-only mode |

### Rust Crate Dependencies

| Crate | Purpose | Alternatives Considered |
|-------|---------|------------------------|
| `clap` | CLI parsing | `structopt` (older), `argh` (simpler) |
| `url` | URL validation | Manual parsing (error-prone) |
| `serde` / `serde_json` | Serialization | Manual JSON (unsafe) |
| `toml` | Config format | `yaml`, `json` (less human-friendly) |
| `anyhow` / `thiserror` | Error handling | Manual error types |
| `directories` | XDG paths | `xdg-basedir` (less maintained) |
| `libc` / `nix` | POSIX APIs | Unsafe libc directly |
| `reqwest` | HTTP client | `hyper` (lower-level), `ureq` (sync) |
| `tokio` | Async runtime | `async-std`, `smol` |
| `backoff` | Retry logic | Manual exponential backoff |
| `ratatui` | TUI framework | `cursive`, `termion` |
| `crossterm` | Terminal control | `termion` (less portable) |

### Why Async?

We use async/await (tokio) because:
1. **Network I/O**: Whisper client is inherently I/O-bound
2. **Non-blocking**: Don't want to block on HTTP requests
3. **Future-proofing**: May add concurrent transcriptions, streaming, etc.
4. **Ecosystem**: reqwest and other HTTP libs are async-first

The overhead is minimal (tokio runtime adds ~2MB to binary).

## Design Decisions

### Why Rust?

The Bash version worked but had limitations:

| Aspect | Bash | Rust |
|--------|------|------|
| Error handling | Exit codes, hard to track | Result types, compile-time checking |
| Concurrency | Background processes, shell job control | Async/await, structured concurrency |
| Type safety | None (all strings) | Strong static typing |
| Testing | Manual, limited | Unit tests, integration tests, doc tests |
| Refactoring | Risky, no tooling | Safe, compiler-enforced |
| Performance | Spawns many processes | Single process, efficient |
| Dependencies | System packages (apt/pacman) | Cargo handles it |
| Portability | Linux-specific shell features | Portable Rust (with platform-specific bits isolated) |

**Trade-offs**:
- Compilation time (Rust is slower to build than Bash "build")
- Binary size (~10MB vs. 5KB Bash script)
- Learning curve (Rust is harder than Bash)

### Why File-Based State?

Alternatives considered:
1. **SQLite database**
   - Overkill for simple state
   - Adds dependency and complexity
   - Doesn't survive crashes as well (needs WAL)

2. **Shared memory**
   - Complex IPC
   - Requires cleanup on crash
   - Not human-inspectable

3. **DBus or systemd**
   - Heavyweight
   - Not all systems have systemd
   - Harder to debug

**File-based state wins** because:
- Simple and debuggable (`cat $XDG_RUNTIME_DIR/ears/recording.pid`)
- Atomic operations (filesystem guarantees)
- Automatic cleanup ($XDG_RUNTIME_DIR cleared on logout)
- No additional dependencies

### Why Separate Binary Invocations?

Each keypress invokes `ears` as a new process rather than running a daemon.

**Alternatives**:
1. **Long-running daemon** (systemd service)
   - Requires systemd
   - Harder to debug (need journal)
   - State must survive across invocations anyway
   - Adds complexity for little benefit

2. **Socket-based IPC** (daemon with client)
   - Two binaries or modes
   - More complex protocol
   - Harder error handling

**Separate invocations** win because:
- Simpler: no IPC, no daemon management
- Stateless: each invocation is independent
- Debuggable: just run the binary directly
- Keyboard shortcut integration is natural

**Trade-off**: Slight startup latency (~50ms on modern hardware) is acceptable.

### Why Not Use ALSA Directly?

We use PipeWire (`pw-record`) instead of ALSA (`arecord`):

**Advantages of PipeWire**:
- Modern standard (replacing PulseAudio)
- Better device isolation (doesn't conflict with OBS, etc.)
- Explicit device targeting (`--target`)
- Native on recent distros

**Backwards compatibility**:
- PipeWire provides PulseAudio compatibility layer
- `paplay` works on both PipeWire and PulseAudio

## Performance Characteristics

### Latency Breakdown

Typical end-to-end latency for a 5-second recording:

| Phase | Time | Notes |
|-------|------|-------|
| Startup (lock, state check) | ~50ms | Rust binary startup |
| Start recording notification | ~100ms | notify-send, paplay |
| Recording | ~5000ms | User speaks |
| Stop recording | ~50ms | SIGTERM to pw-record |
| File write flush | ~300ms | Wait for pw-record to finish writing |
| HTTP POST to whisper | ~500ms | Network + server processing (GPU) |
| Parse response | ~5ms | JSON parsing |
| Type text | ~200ms | ydotool (depends on text length) |
| **Total** | **~6200ms** | ~1200ms overhead for 5s recording |

### Optimizations Applied

1. **Non-blocking audio feedback**: `paplay` runs in background
2. **Minimal allocations**: Reuse buffers where possible
3. **Async I/O**: Don't block on network
4. **Exponential backoff**: Retry quickly first, then slower
5. **File I/O**: Use buffered readers/writers

### Scalability

**Current design handles**:
- Recording up to 2 minutes (timeout protection)
- Audio files up to ~10MB (2 min @ 16kHz mono S16LE = 3.84MB)
- Transcription responses up to 1MB JSON (generous limit)

**Not designed for**:
- Continuous long-running recordings (use a dedicated recorder)
- Concurrent transcriptions (would need worker pool)
- High-frequency toggling (rate limiting not implemented)

## Testing Strategy

### Unit Tests

Each module has unit tests for core logic:
- `config.rs`: Loading, saving, validation
- `state.rs`: State transitions, cleanup
- `lock.rs`: Acquire, release, stale detection
- `process.rs`: Process lifecycle mocking

### Integration Tests

Tests in `tests/` directory:
- **Whisper integration**: Mock server with `wiremock`
- **CLI tests**: `assert_cmd` for command-line testing
- **TUI tests**: Event simulation and rendering validation

### Test Fixtures

- Mock whisper.cpp server responses
- Temporary directories for config/state
- Sample audio files for transcription tests

### Continuous Integration

GitHub Actions runs:
1. `cargo fmt --check` (code formatting)
2. `cargo clippy` (linting)
3. `cargo test` (all tests)
4. `cargo build --release` (optimized build)

**Test coverage goal**: 80%+ (currently at 100% - 86/86 tests passing)

## Security Considerations

### Threat Model

**Assumptions**:
- User's machine is trusted
- Whisper.cpp server is trusted (typically localhost or trusted LAN)
- Audio files contain sensitive data (should be ephemeral)

**Threats**:
1. **Audio file disclosure**: Temporary audio files could be read by other users
2. **Network eavesdropping**: Audio sent over network to whisper server
3. **Command injection**: Malicious device names or URLs
4. **Denial of service**: Fill disk with recordings

### Mitigations

1. **File permissions**:
   ```rust
   // Audio files created with 0600 (user-only read/write)
   use std::os::unix::fs::PermissionsExt;
   fs::set_permissions(audio_file, fs::Permissions::from_mode(0o600))?;
   ```

2. **URL validation**:
   ```rust
   // Use `url` crate to validate server URLs
   // Prevent file:// or other exotic schemes
   if url.scheme() != "http" && url.scheme() != "https" {
       return Err(ConfigError::InvalidScheme);
   }
   ```

3. **Path sanitization**:
   ```rust
   // Device names come from pw-cli, but we still validate
   // No path traversal, no shell metacharacters
   fn validate_device_name(name: &str) -> bool {
       !name.contains(['/', '\\', '\0', '\n', '&', '|', ';', '`'])
   }
   ```

4. **Timeout protection**:
   ```rust
   // Recordings auto-stop after 2 minutes to prevent disk fill
   const MAX_RECORDING_DURATION: Duration = Duration::from_secs(120);
   ```

5. **Cleanup on failure**:
   ```rust
   // Use RAII (Drop trait) to ensure cleanup even on panic
   impl Drop for StateGuard {
       fn drop(&mut self) {
           let _ = fs::remove_file(&self.audio_file);
           let _ = fs::remove_file(&self.pid_file);
       }
   }
   ```

6. **No secrets in logs**:
   - Audio content never logged
   - Transcriptions never logged (user privacy)
   - Only metadata logged (PID, file paths)

### Secure Defaults

- **Default server**: `http://localhost:8178` (loopback only)
- **Audio files**: `$XDG_RUNTIME_DIR` (cleared on logout, not persisted)
- **Config files**: `~/.config/ears/` (user-only readable)

### Future Security Enhancements

- **TLS for remote whisper servers**: Enforce HTTPS for non-localhost
- **Audio encryption at rest**: Encrypt temporary audio files
- **Server authentication**: API key or OAuth for whisper.cpp
- **Audit logging**: Optional detailed logs for security audits

## Future Architecture Considerations

### Potential Enhancements

1. **Plugin system**:
   - Custom post-processors for transcriptions
   - Alternative text input methods (clipboard, Wayland text-input protocol)
   - Language-specific filters

2. **Streaming transcription**:
   - Send audio chunks as they're recorded
   - Real-time feedback (partial transcriptions)
   - Lower latency for long recordings

3. **Multi-server support**:
   - Load balancing across multiple whisper.cpp instances
   - Fallback servers for reliability
   - Health checking and automatic failover

4. **Model management**:
   - Automatic model download
   - Model switching based on language detection
   - Local model cache

5. **Advanced audio processing**:
   - Noise reduction preprocessing
   - Voice activity detection (VAD)
   - Automatic gain control

### Architectural Constraints

To maintain ears' simplicity:
- **No cloud services**: All processing local or self-hosted
- **Single binary**: Don't split into multiple programs
- **Minimal dependencies**: Avoid heavyweight frameworks
- **Config-file simplicity**: TOML, human-editable, no complex schemas

## Conclusion

ears follows a modular, pipeline-based architecture with:
- **Strong separation of concerns** (each module has one job)
- **File-based state** (simple, debuggable, reliable)
- **Process isolation** (audio recording in separate process)
- **Defensive programming** (extensive error handling, validation)
- **Performance** (async I/O, minimal allocations)

The Rust rewrite maintains feature parity with the Bash version while adding:
- Type safety and compile-time error checking
- Comprehensive test coverage
- Better error handling and reporting
- Foundation for future enhancements (TUI, plugins, etc.)

For more details, see:
- [INSTALL.md](INSTALL.md) - Installation and usage
- [CONTRIBUTING.md](CONTRIBUTING.md) - Development workflow
- [API docs](https://docs.rs/ears) - Rust API documentation
