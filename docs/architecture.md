# Architecture Documentation

This document describes the internal architecture of `ears`, implementation details, testing strategy, and contribution guidelines.

## Table of Contents

- [System Architecture](#system-architecture)
- [Component Design](#component-design)
- [State Management](#state-management)
- [Process Flow](#process-flow)
- [Testing Strategy](#testing-strategy)
- [Contributing](#contributing)

## System Architecture

### High-Level Overview

```
┌──────────────────────────────────────────────────────┐
│                  User Interface                      │
│                                                      │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐  │
│  │   CLI    │    │   TUI    │    │  Keyboard    │  │
│  │ Commands │    │  (vim)   │    │  Shortcut    │  │
│  └─────┬────┘    └─────┬────┘    └──────┬───────┘  │
└────────┼───────────────┼─────────────────┼──────────┘
         │               │                 │
         └───────────────┴─────────────────┘
                         │
         ┌───────────────▼───────────────┐
         │      Core Application         │
         │                               │
         │  ┌─────────────────────────┐  │
         │  │   State Machine         │  │
         │  │  (Idle/Recording/Xscr)  │  │
         │  └───────────┬─────────────┘  │
         │              │                │
         │  ┌───────────▼─────────────┐  │
         │  │  Configuration Manager  │  │
         │  │  (Config, Validation)   │  │
         │  └─────────────────────────┘  │
         └───────────────┬───────────────┘
                         │
         ┌───────────────┴───────────────┐
         │                               │
    ┌────▼────┐                   ┌──────▼──────┐
    │ Process │                   │  Whisper    │
    │ Manager │                   │   Client    │
    └────┬────┘                   └──────┬──────┘
         │                               │
    ┌────▼────────┐             ┌────────▼────────┐
    │  PipeWire   │             │  whisper.cpp    │
    │  (pw-record)│             │     Server      │
    └────┬────────┘             └─────────────────┘
         │
    ┌────▼────────┐
    │   Audio     │
    │   Device    │
    └─────────────┘
```

### Layer Architecture

**Presentation Layer (UI):**
- CLI commands (`clap` argument parsing)
- TUI interface (`ratatui` with vim keybindings)
- Keyboard shortcut integration (desktop environment)

**Application Layer (Logic):**
- State machine (validated transitions)
- Configuration management (load/save/validate)
- Error handling and recovery
- Desktop integration (notifications, audio feedback, text input)

**Infrastructure Layer (System):**
- Process management (subprocess spawning, PID tracking, signals)
- File locking (flock-based concurrency control)
- HTTP client (whisper.cpp API)
- PipeWire integration (via `pw-record`)

## Component Design

### Configuration (`src/config.rs`)

**Purpose:** Centralized configuration management

**Structure:**
```rust
pub struct Config {
    pub whisper_server: Url,  // Validated URL (http/https only)
    pub device: String,        // PipeWire device name
    pub config_dir: PathBuf,   // ~/.config/ears
    pub state_dir: PathBuf,    // $XDG_RUNTIME_DIR/ears
}
```

**Key Features:**
- XDG directory standard compliance
- Environment variable overrides (`EARS_SERVER`, `EARS_DEVICE`)
- File-based persistence (`~/.config/ears/{server,device}`)
- URL validation (scheme, host presence)
- Backward compatibility with bash version

**Design Decisions:**
- Used `url::Url` type for type-safe URL handling
- Separated config (persistent) from state (runtime)
- Made validation explicit (not automatic) to give control to caller

### State Management (`src/state.rs`)

**Purpose:** Track and validate recording workflow state

**State Enum:**
```rust
pub enum State {
    Idle,
    Recording { since: SystemTime },
    Transcribing,
}
```

**State Transitions:**
```
     ┌──────┐
     │ Idle │◄─────────────┐
     └───┬──┘              │
         │                 │
    start recording    completed
         │                 │
         ▼                 │
  ┌─────────────┐         │
  │ Recording   ├──stop───┤
  │ {since}     │         │
  └──────┬──────┘         │
         │                │
    stop recording        │
         │                │
         ▼                │
  ┌──────────────┐        │
  │ Transcribing ├────────┘
  └──────────────┘
```

**StateManager:**
```rust
pub struct StateManager {
    state_file: PathBuf,  // Persistent state tracking
}
```

**Key Features:**
- Invalid transition detection (returns `StateError::InvalidTransition`)
- Recording timeout enforcement (2-minute max)
- Emergency stop capability (Recording → Idle allowed)
- Atomic state file updates

**Design Decisions:**
- Recording stores `SystemTime` for timeout calculations
- State file uses simple text format for debuggability
- Emergency transitions allowed to prevent deadlock
- Timeout is checked, not enforced (caller decides action)

### Process Control (`src/process.rs`)

**Purpose:** Safely manage `pw-record` subprocess lifecycle

**ProcessManager:**
```rust
pub struct ProcessManager {
    state_dir: PathBuf,    // Directory for PID file
    pid_file: PathBuf,     // Path to recording.pid
}
```

**Key Features:**
- PID file creation and tracking
- Process health checking (via `kill(pid, 0)`)
- Graceful termination (SIGTERM → wait → SIGKILL)
- Stale PID file detection and cleanup
- Thread-safe signal handling

**Process Lifecycle:**
```
spawn() → [write PID] → [process runs] → stop_process() → [SIGTERM]
                                              ↓
                                         [wait 100ms]
                                              ↓
                                    still running? → [SIGKILL]
                                              ↓
                                         cleanup() → [remove PID file]
```

**Design Decisions:**
- Used `nix` crate for safe signal handling
- SIGTERM first to allow clean shutdown
- 100ms grace period before SIGKILL
- Checks process existence via signal 0 (standard Unix pattern)
- Stale PID detection prevents orphaned files

### Lock Management (`src/lock.rs`)

**Purpose:** Prevent concurrent `ears` instances

**FileLock:**
```rust
pub struct FileLock {
    file: File,  // Holds open file descriptor
}
```

**Key Features:**
- `flock()`-based locking (BSD locks)
- Non-blocking `try_lock()` for check-and-fail
- Blocking `lock()` for wait-and-acquire
- Automatic lock release on drop (RAII pattern)
- Thread-safe (flock is process-wide)

**Design Decisions:**
- Used flock over fcntl (simpler, automatic release on close)
- Lock file kept open while held (prevents removal race)
- Drop trait ensures release even on panic
- Non-blocking variant for user-friendly error messages

### Whisper Integration (`src/whisper.rs`)

**Purpose:** HTTP client for whisper.cpp server API

**WhisperClient:**
```rust
pub struct WhisperClient {
    server_url: Url,
    client: reqwest::Client,
    max_retries: u32,
    retry_delay: Duration,
}
```

**Key Features:**
- Health check endpoint (`/health`)
- Multipart form upload for audio
- JSON response parsing
- Silence artifact filtering
- Exponential backoff retry logic
- rustls-tls (no OpenSSL dependency)

**Transcription Flow:**
```
transcribe() → retry_loop {
    POST /inference
    ↓
    multipart/form-data {
        file: audio.wav,
        response_format: "json"
    }
    ↓
    Parse JSON response
    ↓
    Filter silence artifacts
    ↓
    Return Option<String>
}
```

**Silence Filtering:**
```rust
const SILENCE_ARTIFACTS: &[&str] = &[
    "",
    "Thank you.",
    "Thanks for watching.",
    // ... more patterns
];
```

**Design Decisions:**
- Used `reqwest` with rustls for cross-platform compatibility
- Made retries configurable (default: 3 attempts)
- Returns `Option<String>` (None = silence detected)
- Async API with `tokio` runtime
- Structured errors for different failure modes

### Desktop Integration (`src/desktop.rs`)

**Purpose:** Interact with desktop environment (audio, notifications, text input)

**AudioFeedback:**
- Plays WAV files via `paplay`
- Falls back to system sounds
- Non-blocking (spawns subprocess)

**Notifications:**
- Uses `notify-send` for desktop notifications
- Urgency levels (low, normal, critical)
- Helper methods for common patterns (`error()`, `info()`)

**TextInput:**
- Uses `ydotool` for text automation
- Configurable character delay (default: 12ms)
- Requires `ydotoold` daemon

**Design Decisions:**
- Kept desktop integration separate from core logic
- Used subprocess spawning (not D-Bus) for simplicity
- Made components optional (graceful degradation)
- Synchronous API (blocking calls)

### TUI (`src/tui/`)

**Purpose:** Terminal UI with vim-like controls

**Architecture:**
```
src/tui/
├── mod.rs         # Public API, event loop
├── app.rs         # Application state
├── ui.rs          # Rendering (ratatui)
├── vim_mode.rs    # Vim keybindings
└── command.rs     # Command execution (:q, :w)
```

**Event Loop:**
```
initialize() → event_loop {
    ┌─→ poll_event(timeout=100ms)
    │   ├─ KeyEvent → handle_key()
    │   │   ├─ Normal mode → vim_mode.handle()
    │   │   └─ Command mode → command.handle()
    │   ├─ Resize → update_size()
    │   └─ Tick → refresh_state()
    │
    └─── render() → terminal.draw()
}
```

**Vim Keybindings:**
- `h/l` - Switch tabs
- `j/k` - Scroll logs
- `:q` - Quit
- `:w` - Save config
- `:wq` - Save and quit

**Design Decisions:**
- Separate vim mode state machine
- Command mode with `:` prefix
- Event-driven architecture (not polling)
- State refresh on tick (100ms)

## State Management

### State File Format

State file location: `$XDG_RUNTIME_DIR/ears/state`

Format (plain text):
```
Idle
```
or
```
Recording 1704230400
```
or
```
Transcribing
```

The number after `Recording` is Unix timestamp (seconds since epoch).

**Rationale:** Plain text for:
- Easy debugging (`cat $XDG_RUNTIME_DIR/ears/state`)
- Human readability
- Simple parsing (no JSON overhead)
- Atomic writes (small file size)

### Lock File Strategy

Lock file location: `$XDG_RUNTIME_DIR/ears/lock`

**Concurrency Control:**
1. Process A calls `FileLock::try_lock()`
2. `flock(fd, LOCK_EX | LOCK_NB)` attempted
3. Success → lock acquired, file descriptor held open
4. Failure → another process holds lock
5. Process B tries → gets `LockError::AlreadyLocked`
6. Process A exits → file descriptor closed → lock released

**Why flock?**
- Automatic cleanup on process crash
- Kernel-enforced (no race conditions)
- Works across threads in same process
- Doesn't require explicit unlock

### PID File Management

PID file location: `$XDG_RUNTIME_DIR/ears/recording.pid`

Format:
```
12345
```

**Lifecycle:**
1. `spawn()` creates process
2. Write PID to file
3. Process runs
4. `stop_process()` reads PID, sends signals
5. `cleanup()` removes file

**Stale Detection:**
```rust
pub fn has_stale_pid(&self) -> Result<bool> {
    if let Some(pid) = self.read_pid()? {
        // Send signal 0 (check existence without affecting process)
        Ok(!self.is_pid_alive(pid))
    } else {
        Ok(false)
    }
}
```

## Process Flow

### Recording Start Flow

```
User presses shortcut
    ↓
main() checks current state → Idle
    ↓
Check whisper server health
    ↓
Acquire lock file (try_lock)
    ↓
Create state directory if needed
    ↓
Build pw-record command:
    pw-record --target=<device> <audio_file>
    ↓
ProcessManager::spawn()
    ├─ Start subprocess
    ├─ Write PID to file
    └─ Return PID
    ↓
StateManager::transition_to(Recording { since: now() })
    ↓
AudioFeedback::play_start()
    ↓
Notifications::info("Recording started")
    ↓
Exit (lock remains held)
```

### Recording Stop & Transcribe Flow

```
User presses shortcut
    ↓
main() checks current state → Recording
    ↓
Check recording timeout
    ├─ Exceeded → emergency stop, return
    └─ OK → continue
    ↓
StateManager::transition_to(Transcribing)
    ↓
ProcessManager::stop_process()
    ├─ Send SIGTERM
    ├─ Wait 100ms
    └─ Send SIGKILL if still running
    ↓
Wait 300ms (file flush time)
    ↓
WhisperClient::transcribe(audio_file)
    ├─ Retry loop (up to 3 attempts)
    ├─ POST multipart/form-data
    ├─ Parse JSON response
    └─ Filter silence artifacts → Option<String>
    ↓
Match result:
    ├─ Some(text) → TextInput::type_text(text)
    │               AudioFeedback::play_done()
    │               Notifications::info("Complete")
    │
    └─ None      → Notifications::info("Silence detected")
    ↓
Cleanup:
    ├─ Remove audio file
    ├─ ProcessManager::cleanup() (remove PID)
    └─ StateManager::transition_to(Idle)
    ↓
Exit (lock released)
```

### Emergency Stop Flow

```
User presses shortcut while Recording
    (but whisper server is down or timeout exceeded)
    ↓
Detect condition
    ↓
ProcessManager::stop_process()
    ↓
StateManager::transition_to(Idle)
    ↓
Notifications::error("Reason")
    ↓
AudioFeedback::play_error()
    ↓
Cleanup temp files
    ↓
Exit
```

## Testing Strategy

### Test Coverage Goals

- **Unit tests:** 100% coverage of business logic
- **Integration tests:** Key workflows end-to-end
- **Mock tests:** External dependencies (HTTP, processes)

### Test Organization

```
tests/
├── config_tests.rs          # Configuration loading/saving
├── state_tests.rs           # State transitions, timeouts
├── lock_tests.rs            # Concurrency, deadlock prevention
├── process_tests.rs         # Process lifecycle, signals
├── whisper_tests.rs         # HTTP client, retries, parsing
├── integration/
│   ├── recording_flow.rs    # Full recording workflow
│   └── error_recovery.rs    # Crash recovery, stale state
└── cross-language/
    └── bash_compatibility.rs # Interop with bash version
```

### Key Test Scenarios

#### Configuration Tests
```rust
#[test]
fn test_load_from_files() {
    // Test loading from ~/.config/ears/server and device
}

#[test]
fn test_env_override() {
    // Test EARS_SERVER and EARS_DEVICE override files
}

#[test]
fn test_url_validation() {
    // Test invalid schemes (ftp://, file://) are rejected
}
```

#### State Transition Tests
```rust
#[test]
fn test_valid_transitions() {
    // Idle → Recording → Transcribing → Idle
}

#[test]
fn test_invalid_transitions() {
    // Idle → Transcribing should fail
}

#[test]
fn test_emergency_stop() {
    // Recording → Idle should succeed (emergency case)
}

#[test]
fn test_timeout_detection() {
    // Recording for >2 minutes should be detected
}
```

#### Lock Tests
```rust
#[test]
fn test_concurrent_lock() {
    // Two threads try to acquire same lock
    // Second should fail with AlreadyLocked
}

#[test]
fn test_lock_release_on_drop() {
    // Lock should be released when FileLock drops
}

#[test]
fn test_lock_survives_panic() {
    // Lock should be released even if thread panics
}
```

#### Process Tests
```rust
#[test]
fn test_spawn_and_stop() {
    // Spawn sleep process, verify PID file, stop it
}

#[test]
fn test_stale_pid_detection() {
    // Create PID file with non-existent PID
    // has_stale_pid() should return true
}

#[test]
fn test_graceful_shutdown() {
    // SIGTERM should be tried before SIGKILL
}
```

#### Whisper Client Tests (using wiremock)
```rust
#[tokio::test]
async fn test_successful_transcription() {
    // Mock server returns {"text": "hello"}
    // Should return Some("hello")
}

#[tokio::test]
async fn test_silence_filtering() {
    // Mock server returns {"text": "Thank you."}
    // Should return None
}

#[tokio::test]
async fn test_retry_on_failure() {
    // Mock server fails twice, succeeds third time
    // Should eventually succeed
}

#[tokio::test]
async fn test_health_check() {
    // Mock /health endpoint
    // Should return Ok(())
}
```

### Testing Tools

**Dependencies:**
```toml
[dev-dependencies]
tempfile = "3.16"       # Temporary directories for state
serial_test = "3.2"     # Sequential test execution (env vars)
tokio-test = "0.4"      # Async test utilities
wiremock = "0.6"        # HTTP mocking for whisper client
assert_cmd = "2.0"      # CLI testing
predicates = "3.1"      # Assertion helpers
```

**Common Patterns:**

Temporary state directories:
```rust
use tempfile::TempDir;

#[test]
fn test_something() {
    let temp = TempDir::new().unwrap();
    let state_dir = temp.path();
    // ... use state_dir ...
    // Automatically cleaned up when temp drops
}
```

Sequential environment variable tests:
```rust
use serial_test::serial;

#[test]
#[serial]  // Prevents parallel execution
fn test_env_var() {
    std::env::set_var("EARS_SERVER", "http://test:8178");
    // ... test ...
    std::env::remove_var("EARS_SERVER");
}
```

Mock HTTP servers:
```rust
use wiremock::{MockServer, Mock, ResponseTemplate};
use wiremock::matchers::{method, path};

#[tokio::test]
async fn test_http() {
    let mock_server = MockServer::start().await;
    
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(json!({"text": "hello"})))
        .mount(&mock_server)
        .await;
    
    let client = WhisperClient::new(mock_server.uri())?;
    let result = client.transcribe(audio).await?;
    assert_eq!(result, Some("hello".to_string()));
}
```

### Running Tests

```bash
# All tests
cargo test

# Specific module
cargo test whisper

# Integration tests only
cargo test --test integration

# With output
cargo test -- --nocapture

# Single test
cargo test test_valid_transitions -- --exact
```

### Continuous Integration

GitHub Actions workflow (`.github/workflows/ci.yml`):
```yaml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --all-features
      - run: cargo clippy -- -D warnings
      - run: cargo fmt -- --check
```

## Contributing

### Development Setup

1. **Clone the repository:**
   ```bash
   git clone https://github.com/heiervang-technologies/ears
   cd ears
   ```

2. **Install Rust (if not already installed):**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. **Install development dependencies:**
   ```bash
   # Ubuntu/Debian
   sudo apt install pipewire ydotool wl-clipboard libnotify-bin pulseaudio-utils fzf
   
   # Start ydotool daemon
   ydotoold &
   ```

4. **Run tests:**
   ```bash
   cargo test
   ```

5. **Build the project:**
   ```bash
   cargo build --release
   ```

### Code Style

- **Formatting:** Use `rustfmt`
  ```bash
  cargo fmt
  ```

- **Linting:** Use `clippy`
  ```bash
  cargo clippy -- -D warnings
  ```

- **Naming conventions:**
  - `snake_case` for functions and variables
  - `PascalCase` for types and traits
  - `SCREAMING_SNAKE_CASE` for constants

- **Error handling:**
  - Use `Result<T, E>` for fallible functions
  - Use `thiserror` for library errors
  - Use `anyhow` for application errors
  - Provide context with `.context("description")`

- **Documentation:**
  - Document all public APIs with `///` comments
  - Include examples in doc comments
  - Add module-level documentation (`//!`)

### Pull Request Process

1. **Create a feature branch:**
   ```bash
   git checkout -b feature/my-feature
   ```

2. **Make your changes:**
   - Write code
   - Add tests
   - Update documentation
   - Run `cargo fmt` and `cargo clippy`

3. **Commit with conventional commits:**
   ```
   feat: Add support for multiple audio formats
   fix: Handle whisper server timeout correctly
   docs: Update API documentation for WhisperClient
   test: Add integration test for recording flow
   refactor: Simplify state transition logic
   ```

4. **Push and create PR:**
   ```bash
   git push origin feature/my-feature
   ```
   
   Then open a pull request on GitHub.

5. **CI checks must pass:**
   - All tests passing
   - No clippy warnings
   - Formatting correct

6. **Code review:**
   - Address reviewer feedback
   - Update PR as needed

7. **Merge:**
   - Squash commits if requested
   - Rebase on main if needed

### Adding New Features

#### Example: Adding a New Audio Format

1. **Update `RecordingConfig` in `src/recording.rs`:**
   ```rust
   pub enum AudioFormat {
       S16,    // Existing
       F32,    // New format
   }
   ```

2. **Update command building:**
   ```rust
   fn build_command(&self) -> Command {
       // ...
       match self.format {
           AudioFormat::S16 => cmd.arg("--format=s16"),
           AudioFormat::F32 => cmd.arg("--format=f32"),
       }
       // ...
   }
   ```

3. **Add tests:**
   ```rust
   #[test]
   fn test_f32_format() {
       let config = RecordingConfig {
           format: AudioFormat::F32,
           // ...
       };
       let cmd = config.build_command();
       // assert command args
   }
   ```

4. **Update documentation:**
   - Add to API docs
   - Update user guide
   - Add example

### Debugging

#### Enable tracing logs:
```bash
RUST_LOG=ears=debug cargo run
```

#### Check state files:
```bash
ls -la $XDG_RUNTIME_DIR/ears/
cat $XDG_RUNTIME_DIR/ears/state
cat $XDG_RUNTIME_DIR/ears/recording.pid
```

#### Test whisper server manually:
```bash
curl -X POST http://localhost:8178/inference \
  -F "file=@test.wav" \
  -F "response_format=json" | jq
```

#### Attach to running process:
```bash
# Find PID
cat $XDG_RUNTIME_DIR/ears/recording.pid

# Monitor with strace
strace -p <PID>
```

### Common Development Tasks

#### Adding a new CLI command:
1. Update `src/cli.rs` with new subcommand
2. Add handler in `src/main.rs`
3. Add tests
4. Update help text and documentation

#### Adding a new state:
1. Add variant to `State` enum in `src/state.rs`
2. Update `transition_to()` to validate new transitions
3. Update state file parsing
4. Add tests for new transitions

#### Updating whisper API:
1. Modify `WhisperClient` in `src/whisper.rs`
2. Update request building
3. Update response parsing
4. Add wiremock tests
5. Update API documentation

### Release Process

1. **Update version in `Cargo.toml`**
2. **Update CHANGELOG.md**
3. **Create git tag:**
   ```bash
   git tag -a v0.2.0 -m "Release v0.2.0"
   git push origin v0.2.0
   ```
4. **CI builds release artifacts**
5. **Create GitHub release** with changelog

### Getting Help

- **Documentation:** Read [user-guide.md](user-guide.md) and [api.md](api.md)
- **Issues:** [GitHub Issues](https://github.com/heiervang-technologies/ears/issues)
- **Discussions:** [GitHub Discussions](https://github.com/heiervang-technologies/ears/discussions)
- **Code Review:** Tag maintainers in your PR

## Architectural Decisions

### Why Rust?
- Type safety prevents whole classes of bugs
- No garbage collector (predictable performance)
- Excellent async support (tokio ecosystem)
- Strong ecosystem (serde, clap, reqwest, etc.)
- Cross-compilation for different targets

### Why PipeWire?
- Modern Linux audio stack
- Low latency
- Better device management than ALSA
- Replaces PulseAudio on newer systems

### Why whisper.cpp over other STT solutions?
- GPU-accelerated, very fast
- Runs locally (privacy)
- No API costs
- State-of-the-art accuracy (OpenAI Whisper)
- Simple HTTP API

### Why ydotool over xdotool?
- Works on Wayland (xdotool is X11 only)
- Modern input automation
- Permission-based security

### Why file-based state over database?
- Simplicity (no SQLite dependency)
- Debuggability (text files, human-readable)
- Atomicity (small files, fast writes)
- Performance (no query overhead)
- Matches bash version behavior

### Why flock over fcntl?
- Simpler API
- Automatic cleanup on process exit
- BSD lock semantics (clearer)
- No need for explicit unlock

## Future Architecture Considerations

### Potential Improvements

1. **Plugin System:**
   - Allow custom transcription backends
   - Pluggable text input methods
   - Custom silence detection

2. **Multiple Audio Formats:**
   - Support FLAC, Opus for better compression
   - Configurable sample rates

3. **Advanced Retry Strategies:**
   - Circuit breaker pattern for whisper client
   - Backpressure handling

4. **Observability:**
   - Metrics export (Prometheus)
   - Structured logging (tracing)
   - Health check endpoint

5. **Configuration Hot-reload:**
   - Watch config files for changes
   - Reload without restart

6. **Multi-language Support:**
   - Automatic language detection
   - Multiple whisper models

## License

MIT License - See LICENSE file for details.

## Credits

Developed by Mark Sverdhei (@marksverdhei, @marksverdhai)

Built with:
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) - Whisper inference
- [PipeWire](https://pipewire.org/) - Audio routing
- [ydotool](https://github.com/ReimuNotMoe/ydotool) - Input automation
- [ratatui](https://github.com/ratatui-org/ratatui) - TUI framework
