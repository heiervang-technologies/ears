# ears Architecture

This document describes the internal architecture of ears, a speech recognition daemon for Linux. It covers the major subsystems, data flows, and design decisions.

## 1. System Overview

ears is a Rust application that captures audio from PipeWire, sends it to a whisper.cpp-compatible ASR server for transcription, and types the resulting text into the focused window. It operates in four modes:

- **TUI mode** (`ears`) -- Interactive terminal UI with VAD controls, configuration, and live status.
- **Push-to-talk** (`ears toggle`) -- Keybind-driven: press once to start recording, press again to stop and transcribe.
- **Headless VAD** (`ears vad`) -- Continuous listening with automatic speech detection and transcription.
- **WebSocket input** (`ears ws-listen`) -- Receives audio over WebSocket instead of from a local microphone. Runs alongside the desktop instance on a separate IPC socket.

The major subsystems are:

```
                   +------------------+
                   |   CLI (clap)     |
                   +--------+---------+
                            |
             +--------------+--------------+
             |              |              |
        ears (TUI)    ears toggle    ears vad / ws-listen
             |              |              |
     +-------+-------+     |       +------+------+
     |  TUI App/UI   |     |       | Headless    |
     |  (ratatui)    |     |       | event loop  |
     +-------+-------+     |       +------+------+
             |              |              |
     +-------+-------+     |       +------+------+
     |  VAD Pipeline  |    |       | VAD Pipeline |
     |  (streaming    |    |       | (streaming   |
     |   engine)      |    |       |  engine)     |
     +-------+-------+     |       +------+------+
             |              |              |
     +-------+-------+  +--+--+   +-------+------+
     | ContinuousCapt|  | pw- |   | ContinuousCapt|
     | (pw-record)   |  |record|  | (pw-record)   |
     +-------+-------+  +--+--+   +-------+------+
             |              |              |
     +-------+--------------+--------------+------+
     |              Whisper HTTP Client            |
     |   (POST /v1/audio/transcriptions)           |
     +---------------------------------------------+
             |
     +-------+-------+
     |  Text Input    |
     | (wtype/ydotool)|
     +----------------+
```

## 2. State Machine

The system state is tracked in `src/state.rs` via the `State` enum and `StateManager`.

### States

| State | Description |
|-------|-------------|
| `Idle` | No active recording. Ready for input. |
| `Recording` | Push-to-talk mode: `pw-record` is capturing audio. |
| `Transcribing` | Audio has been sent to the whisper server; waiting for response. |
| `VadActive` | VAD pipeline is running (continuous listening). |

### Valid Transitions

```
Idle --> Recording        (push-to-talk: start)
Recording --> Transcribing (push-to-talk: stop)
Transcribing --> Idle      (transcription complete or error)
Idle --> VadActive         (VAD mode activated)
VadActive --> Idle         (VAD mode deactivated)
* --> Idle                 (emergency stop, always allowed)
```

Invalid transitions (e.g., `Idle --> Transcribing` or `Recording --> Recording`) return `StateError::InvalidTransition`.

### Persistence

State is persisted to `$XDG_RUNTIME_DIR/ears/state` as a plain text string (`idle`, `recording`, `transcribing`, `vad_active`). On each transition, `pkill -RTMIN+9 waybar` is called to refresh the waybar ears indicator.

### Crash Recovery

- A `TranscribingGuard` (drop guard) resets state to `Idle` if the process panics or returns early during transcription.
- `StateManager::reconcile_state()` detects stale `Recording` or `Transcribing` states on startup (e.g., after a crash) and resets to `Idle`.
- Recording has a 2-minute timeout enforced by `StateManager`.

### Concurrency Control

- `FileLock` prevents multiple TUI instances from running simultaneously.
- A per-toggle `toggle.lock` serializes rapid keybind presses to prevent race conditions on the state file.
- VAD mode uses `vad.lock` and a PID file (`vad.pid`) to ensure only one VAD instance runs.

## 3. Audio Pipeline

### Push-to-Talk Mode

1. `ProcessManager::spawn_recording()` launches `pw-record` with `timeout` wrapper (120s max):
   ```
   timeout 120 pw-record --target <device> recording.wav
   ```
2. Audio is written to `$XDG_RUNTIME_DIR/ears/recording.wav` as a standard WAV file (16-bit PCM).
3. On stop, `pw-record` receives SIGTERM, and ears waits 300ms for the file to flush.
4. The WAV file is validated (RIFF header check, minimum size beyond `WAV_HEADER_SIZE` of 78 bytes).
5. The file is POSTed to the whisper server, then deleted.

### Continuous Capture (VAD / TUI VAD mode)

`ContinuousCapture` (in `src/continuous_capture.rs`) runs `pw-record` in streaming mode, outputting raw PCM to stdout:

```
pw-record --target <device> --rate 16000 --channels 1 --format s16 -
```

A `spawn_blocking` reader thread reads 1600-sample chunks (100ms at 16kHz) from stdout, converts `i16` to `f32` (normalized to -1.0..1.0), and sends them through an unbounded `mpsc` channel to the streaming engine.

### WebSocket Input

`ws-listen` mode (`src/ws_input.rs`) accepts WebSocket connections instead of using `pw-record`. Clients send a `{"type": "start", ...}` text frame followed by binary frames of raw s16le PCM audio. The audio is converted to `f32` and fed into the same streaming engine pipeline. Events are echoed back to the WebSocket client as JSON text frames. This mode uses a separate IPC socket (default `$XDG_RUNTIME_DIR/ears-ws.sock`) to avoid conflicting with the desktop instance. Clients can override the socket path with `--socket`.

## 4. VAD Pipeline

The VAD pipeline detects speech segments in continuous audio and transcribes each one. It is used by both the TUI and headless `ears vad` mode.

### Components

```
ContinuousCapture --> StreamingEngine
                          |
                  +-------+-------+
                  |               |
            VadSegmentDetector  AudioBuffer
                  |
              SileroVad (ONNX)
                  |
            SpeechSegment
                  |
            WhisperClient.transcribe()
                  |
            TextFilters.apply()
                  |
            LocalAgreementPolicy
                  |
            ProgressiveTypingEngine
                  |
            TextInput (wtype/ydotool)
```

### Silero VAD (`src/vad.rs`)

Uses the Silero VAD v5 neural network model via ONNX Runtime (`voice_activity_detector` crate). Key parameters:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `speech_threshold` | 0.5 | Probability threshold for speech detection (0.0-1.0) |
| `min_speech_duration_ms` | 300 | Minimum speech duration before a segment starts |
| `max_silence_duration_ms` | 700 | Silence duration that ends a segment |
| `pre_speech_buffer_ms` | 500 | Ring buffer of recent audio prepended to segments to avoid clipping utterance onsets |

The model operates on fixed 512-sample frames (32ms at 16kHz). `VadSegmentDetector` handles reframing: incoming 1600-sample chunks from `ContinuousCapture` are buffered and processed in exact 512-sample frames. Any remainder is kept for the next call.

### Speech Detection States

The VAD tracks two levels of speech detection:

- **Probable speech** (`is_probably_speaking`): Speech frames are accumulating but haven't yet met the `min_speech_duration_ms` threshold. Emits `SpeechProbable` event.
- **Confirmed speech** (`is_speaking`): Duration threshold met. Emits `SpeechStarted` event.
- **Speech ended**: Silence has persisted beyond `max_silence_duration_ms`. Emits `SpeechEnded` event and yields a `SpeechSegment` with the collected audio samples.

### Pre-speech Replay Buffer

During silence, audio frames are accumulated in a ring buffer (`VecDeque`) with capacity determined by `pre_speech_buffer_ms`. When speech is detected, the buffer contents are prepended to the segment so the beginning of the utterance is not clipped.

## 5. Streaming / Progressive Typing

### LocalAgreementPolicy (`src/streaming.rs`)

Ensures only stable text is committed. The policy keeps a history of the last `n` transcription results (default `n=2`) and commits only the longest common prefix across all of them.

Example flow:
1. Transcript 1: `"Hello"` -- not enough history, nothing committed.
2. Transcript 2: `"Hello wo"` -- common prefix is `"Hello"`, commit `"Hello"`.
3. Transcript 3: `"Hello world"` -- common prefix with previous is `"Hello wo"`, commit `" wo"`.

In the current VAD segment-based approach, each segment is a discrete utterance. The agreement state is reset between segments, and the transcript is fed twice to force immediate commitment.

### Progressive Typing (`src/progressive_typing.rs`)

`ProgressiveTypingEngine` tracks what text has been physically typed and computes diffs:

- **Append**: If the new committed text extends the previously typed text, only the new suffix is typed.
- **Correction** (when `auto_correction` is enabled): If the committed text diverges from what was typed, backspace characters are sent to delete the divergent portion, then the correct text is typed.
- **No correction**: If correction is disabled and text diverges, the new suffix is appended (may result in incorrect text but is faster).

### StreamingEngine (`src/streaming_engine.rs`)

The `StreamingEngine` is the central coordinator. It owns all pipeline components and processes audio in its `process_audio()` method:

1. Writes samples to `AudioBuffer` (circular buffer, configurable duration).
2. Feeds samples to `VadSegmentDetector`.
3. When a complete `SpeechSegment` is returned:
   - Saves the segment audio to a temporary WAV file.
   - Transcribes via `WhisperClient`.
   - Applies `TextFilters` (lowercase, punctuation removal, strict alphabet).
   - Feeds through `LocalAgreementPolicy` for text stabilization.
   - Sends to `ProgressiveTypingEngine` for output.
   - Optionally sends Enter key after each segment (`auto_enter`).
4. Emits `StreamingEvent`s to listeners (TUI, IPC clients).

### StreamingEvent

Events emitted by the engine:

| Event | Description |
|-------|-------------|
| `SpeechProbable` | First speech frames detected (before min duration met) |
| `SpeechStarted` | Speech confirmed (min duration threshold met) |
| `SpeechEnded` | Speech segment ended |
| `TranscriptUpdate { committed, uncommitted }` | Text update with stable and unstable portions |
| `SegmentCompleted { text, duration_ms }` | Full segment transcribed |
| `Error(String)` | Error in the pipeline |
| `StatsUpdate { segments_processed, avg_latency_ms }` | Performance statistics |

## 6. IPC Protocol

ears exposes two Unix domain sockets for inter-process communication.

### Event Socket (read-only broadcast)

**Path**: `$XDG_RUNTIME_DIR/ears.sock` (or `/tmp/ears.sock` fallback)

Broadcasts `StreamingEvent`s as newline-delimited JSON (NDJSON). Each connected client receives all events via a `tokio::sync::broadcast` channel. Clients that fall behind receive a "lagged" notification and skip missed events.

Protocol: connect, then read lines. Each line is a JSON-serialized `StreamingEvent`.

### Command Socket (bidirectional)

**Path**: `$XDG_RUNTIME_DIR/ears-cmd.sock` (or `/tmp/ears-cmd.sock` fallback)

Accepts line-delimited text commands and returns a response.

Currently supported commands:

| Command | Response | Description |
|---------|----------|-------------|
| `toggle-auto-enter` | `auto-enter:on` or `auto-enter:off` | Toggle the auto-enter setting |
| (unknown) | `error:unknown-command` | Any unrecognized command |

The `ears auto-enter` CLI command uses `ipc::send_command("toggle-auto-enter")` to communicate with the running instance.

### WebSocket IPC

When running in `ws-listen` mode, events are also echoed back to connected WebSocket clients as JSON text frames. The WS server uses a separate IPC socket path (default `$XDG_RUNTIME_DIR/ears-ws.sock`) to avoid conflicts with the desktop instance. Integrations can specify a custom socket path via the `--socket` flag.

## 7. TUI Architecture

The TUI is built with ratatui (rendering) and crossterm (terminal I/O).

### Event Loop

```
EventHandler (background thread)
  |-- polls crossterm events at 250ms tick rate
  |-- sends Event::Key, Event::Mouse, Event::Resize, Event::Tick
  |-- receives Event::ModelFetched from async health check
  v
Main loop (async):
  1. Clear clickable regions
  2. Draw UI (ui::render)
  3. Handle next event (key/mouse/tick/resize)
  4. Drain IPC commands (toggle-auto-enter, etc.)
  5. Push typing settings to engine if changed
  6. Drain streaming events and broadcast to IPC
  7. Check if VAD state changed, start/stop pipeline
```

### App State (`src/tui/app.rs`)

`App` holds all TUI state:

- **Panels**: `Status`, `Logs`, `Config`, `Help` -- navigated with `Tab`/`Shift-Tab` or `1`-`4` keys.
- **VAD state**: `vad_active`, `is_speaking`, `committed_text`, `uncommitted_text`, stats.
- **Settings**: `progressive_typing`, `auto_correction`, `typing_mode`, `auto_enter`, `text_filters`.
- **Clickable regions**: Rectangles registered during rendering for mouse interaction.
- **Log buffer**: Ring buffer of log entries with timestamps, filterable by level (All/Errors/Warnings).

Key bindings: `v` toggles VAD, `p` toggles progressive typing, `c` toggles auto-correction, `m` cycles typing mode, `e` toggles auto-enter, `l`/`r`/`a` toggle text filters.

### Rendering (`src/tui/ui.rs`)

Uses ratatui's layout system with a header bar (tabs), main content area (panel-dependent), and a footer. The Status panel shows connection info, VAD status with visual speaking indicator, committed/uncommitted text, and performance stats.

### VAD Pipeline Integration

When the user toggles VAD in the TUI, `start_vad_pipeline()` is called which:
1. Creates a `WhisperClient` with language detection.
2. Starts `ContinuousCapture` with the configured audio device.
3. Creates a `StreamingEngine` with VAD config from settings.
4. Returns a shutdown sender, settings sender (watch channel), and join handle.

Settings changes in the TUI (progressive typing, auto-correction, typing mode, auto-enter, text filters) are pushed to the engine via the `watch::Sender<TypingSettings>` channel.

## 8. Text Input

Transcribed text is typed into the focused window using one of several methods, selected by the `TypingMode` setting.

### Modes

| Mode | Method | When Used |
|------|--------|-----------|
| `Auto` | Auto-detect | Default. Uses `wtype` on Omarchy/Hyprland, clipboard paste otherwise. |
| `Wtype` | `wtype -d 4 -- <text>` | Direct Wayland text input. 4ms inter-key delay prevents browsers from dropping characters. |
| `Paste` | `wl-copy` + `ydotool key ctrl+v` | Clipboard-based paste. Saves/restores original clipboard. |
| `None` | No output | Disables typing entirely (useful for IPC-only consumers). |

### Omarchy Detection

`TextInput::is_omarchy()` checks for the presence of both `hyprctl` and `wtype` to determine if the system is running Hyprland with wtype available.

### Enter Key

When `auto_enter` is enabled, `TextInput::send_enter()` uses `ydotool key 28:1 28:0` (kernel-level KEY_ENTER press/release) after typing. This is used instead of wtype because ydotool creates kernel-level evdev events that work reliably in TUI apps and tmux.

### Keyboard Layout Detection

`KeyboardLayout::detect_language()` attempts to determine the current keyboard layout for language-aware transcription:

1. **Hyprland**: Queries `hyprctl devices -j` for the `active_keymap` field, parsing names like "English (US)" or "Norwegian" into layout codes.
2. **GNOME**: Reads `dconf /org/gnome/desktop/input-sources/mru-sources` for the most recently used keyboard layout.
3. Maps layout codes to language codes (e.g., `us` -> `en`, `no` -> `no`, `de` -> `de`).

## 9. Configuration

### Config File

Configuration is stored in TOML format at `~/.config/ears/config.toml`:

```toml
server = "http://127.0.0.1:8178"    # Whisper server URL
device = "default"                   # PipeWire audio device name
# language = "en"                    # Language code (auto-detected from keyboard if omitted)
# api_key = "sk-..."                 # API key for authenticated ASR services
# model = "whisper-large-v3-turbo"   # Model name (for cloud APIs)
typing_mode = "auto"                 # auto | wtype | paste | none
auto_enter = true                    # Send Enter after each transcription
progressive_typing = false           # Type text as it becomes stable
# auto_correction = true             # Backspace and retype on corrections (defaults to progressive_typing value)
cue_volume = 100                     # Audio cue volume (0-100)

[text_filters]
lowercase = false                    # Convert output to lowercase
remove_punctuation = false           # Strip punctuation
strict_alphabet = true               # Reject text with too many foreign-script characters
alphabet_threshold = 0.5             # Foreign character proportion threshold (0.0-1.0)

[vad]
speech_threshold = 0.5               # VAD speech probability threshold (0.0-1.0)
min_speech_duration_ms = 300         # Minimum speech duration before segment starts
max_silence_duration_ms = 700        # Silence duration that ends a segment
pre_speech_buffer_ms = 500           # Pre-speech replay buffer duration
```

### Profile System

Named profiles allow switching between ASR backends. Profile configs are named `config.<name>.toml` in the same directory.

**Resolution order** (highest priority first):
1. CLI flag: `ears -p groq`
2. Environment variable: `EARS_PROFILE=groq`
3. Persisted default: `~/.config/ears/profile` (plain text file with profile name)
4. Default: `config.toml`

### Environment Variable Overrides

Environment variables override config file values (applied after file load):

| Variable | Overrides |
|----------|-----------|
| `EARS_SERVER` | `server` |
| `EARS_DEVICE` | `device` |
| `EARS_LANGUAGE` | `language` |
| `EARS_API_KEY` | `api_key` |
| `EARS_MODEL` | `model` |
| `EARS_PROFILE` | Profile selection |

### Precedence Summary

```
CLI flags > Environment variables > Config file > Defaults
```

### Migration

On first run, if `config.toml` does not exist, ears checks for legacy single-file configs (`server`, `device`, `language`, `api_key`, `model`, `text_filters.json`) and migrates them into a new `config.toml`.

## 10. External Dependencies

### Runtime Binaries

| Binary | Package (Arch) | Used By | Purpose |
|--------|---------------|---------|---------|
| `pw-record` | `pipewire` | `ContinuousCapture`, `ProcessManager` | Audio capture (streaming or file) |
| `pw-cli` | `pipewire` | `audio.rs` | Audio device discovery and listing |
| `timeout` | `coreutils` | `ProcessManager` | Recording duration limit (wraps pw-record) |
| `pkill` | `procps-ng` | `StateManager` | Signal waybar on state changes (SIGRTMIN+9) |
| `notify-send` | `libnotify` | `Notifications` | Desktop notifications |
| `paplay` | `libpulse` | `AudioFeedback` | Play audio cue sounds |
| `wtype` | `wtype` | `TextInput` | Direct Wayland text input (Hyprland) |
| `ydotool` | `ydotool` | `TextInput` | Keyboard simulation (Enter key, clipboard paste) |
| `hyprctl` | `hyprland` | `KeyboardLayout`, `TextInput` | Keyboard layout detection, Omarchy detection |
| `dconf` | `dconf` | `KeyboardLayout` | GNOME keyboard layout detection |
| `wl-copy` / `wl-paste` | `wl-clipboard` | `TextInput` | Clipboard operations (non-Omarchy paste path) |
| `fzf` | `fzf` | `audio.rs` | Interactive device selection |
| `column` | `util-linux` | `main.rs` | Device list formatting |

### Rust Crates (key dependencies)

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime |
| `ratatui` + `crossterm` | TUI rendering and terminal I/O |
| `reqwest` (rustls) | HTTP client for whisper API |
| `voice_activity_detector` | Silero VAD ONNX model |
| `clap` | CLI argument parsing |
| `serde` + `toml` | Configuration serialization |
| `tracing` | Structured logging |
| `anyhow` / `thiserror` | Error handling |
| `nix` | Unix signal handling (SIGTERM for VAD stop) |
| `tokio-tungstenite` | WebSocket server |

### External Service

ears requires a running ASR server that implements the OpenAI-compatible `/v1/audio/transcriptions` endpoint. Compatible servers include:

- whisper.cpp server
- faster-whisper server
- Groq API (cloud)
- Any OpenAI-compatible ASR endpoint

## 11. Error Handling

### Conventions

- **Binary code** (`main.rs`, TUI): Uses `anyhow::Result` for ergonomic error propagation with context via `.context()`.
- **Library modules** (`state.rs`, `vad.rs`, `streaming.rs`, etc.): Uses `thiserror` for typed error enums that implement `std::error::Error`.

### Error Types

| Module | Error Type | Key Variants |
|--------|-----------|--------------|
| `state` | `StateError` | `InvalidTransition`, `RecordingTimeout`, `CorruptedState` |
| `vad` | `VadError` | `InvalidAudio`, `ConfigError`, `ModelError` |
| `streaming` | `StreamingError` | `BufferOverflow`, `BackendError`, `VadError` |
| `streaming_engine` | `StreamingEngineError` | `VadError`, `TranscriptionError`, `AudioError` |
| `continuous_capture` | `ContinuousCaptureError` | `StartError`, `ProcessDied`, `ReadError` |
| `progressive_typing` | `ProgressiveTypingError` | `TextInputError`, `InvalidState` |
| `whisper` | `WhisperError` | HTTP and transcription errors |
| `lock` | `LockError` | File locking errors |
| `process` | `ProcessError` | Process spawn/management errors |

### Recovery Patterns

- **Drop guards**: `TranscribingGuard` and `StateCleanupGuard` ensure state resets to `Idle` even on panic. `ContinuousCapture` kills `pw-record` on drop.
- **Stale state reconciliation**: On startup, `Recording` state with no live process or `Transcribing` state is reset to `Idle`.
- **Graceful degradation**: Missing optional tools (e.g., `column`, `fzf`, `notify-send`) are handled with fallbacks or silent failures (`.ok()`).
- **Health checks**: The whisper server is health-checked before starting a recording or VAD pipeline.

### Logging

Logs are written to `$XDG_RUNTIME_DIR/ears/debug.log` using `tracing_subscriber` with file output (no ANSI codes). Log level is controlled by `RUST_LOG` env var (default: `info`).

## 12. Post-Transcribe Hook

An optional executable at `~/.config/ears/hooks/post-transcribe` is called after each successful transcription with:
- `$1` -- Path to a copy of the audio file (the copy is made so the hook can process it asynchronously)
- `$2` -- The transcribed text

The hook runs in a background thread (fire-and-forget) with stdin/stdout/stderr redirected to null.
