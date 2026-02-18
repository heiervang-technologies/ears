# API Documentation

The `ears` library API for embedding speech recognition into Rust applications.

## Overview

```toml
[dependencies]
ears = "1.1"
tokio = { version = "1", features = ["full"] }
```

## Core Types

### Config

```rust
use ears::Config;

// Load from files + env vars
let config = Config::load()?;

// Fields:
// config.whisper_server  - Url
// config.device          - String
// config.language        - Option<String>
// config.text_filters    - TextFilters
// config.config_dir      - PathBuf
// config.state_dir       - PathBuf

config.save()?;
config.validate()?;
```

Config files: `~/.config/ears/{server,device,language,text_filters.json}`

Environment overrides: `EARS_SERVER`, `EARS_DEVICE`, `EARS_LANGUAGE`

### WhisperClient

```rust
use ears::WhisperClient;

let client = WhisperClient::new("http://localhost:8178");

// Health check
client.health_check().await?;

// Transcribe (returns String, errors on silence/empty)
let text = client.transcribe("/path/to/recording.wav").await?;

// With language
let client = WhisperClient::new("http://localhost:8178")
    .with_language(Some("en".to_string()));

// Custom retry config
let client = WhisperClient::with_retry_config(
    "http://localhost:8178",
    5,      // max_retries
    200,    // initial_backoff_ms
    10000,  // max_backoff_ms
);
```

Uses `/v1/audio/transcriptions` endpoint (OpenAI-compatible). Automatically filters silence artifacts.

### State Management

```rust
use ears::{State, StateManager};

let mut mgr = StateManager::new("/tmp/ears")?;
mgr.load_state()?;

match mgr.current_state() {
    State::Idle => { /* ready */ }
    State::Recording => { /* recording */ }
    State::Transcribing => { /* processing */ }
    State::VadActive => { /* VAD mode */ }
}

mgr.transition(State::Recording)?;
mgr.transition(State::Transcribing)?;
mgr.transition(State::Idle)?;
```

### File Locking

```rust
use ears::FileLock;

let lock = FileLock::try_lock("/tmp/ears/lock")?;
// Lock released on drop
```

### Process Management

```rust
use ears::ProcessManager;
use std::time::Duration;

let mgr = ProcessManager::new(&pid_file, Duration::from_secs(120));
let pid = mgr.spawn_recording(&device, &audio_file)?;
mgr.is_recording_alive()?;
mgr.stop_recording()?;
mgr.cleanup_stale()?;
```

### Desktop Integration

```rust
use ears::{AudioFeedback, Notifications, TextInput, KeyboardLayout};

AudioFeedback::beep_start()?;
AudioFeedback::beep_done()?;
AudioFeedback::beep_error()?;

Notifications::info("Recording started")?;
Notifications::error("Server down")?;

TextInput::type_text("Hello world")?;

let lang = KeyboardLayout::detect_language(); // Option<String>
```

### Text Filters

```rust
use ears::TextFilters;

let filters = TextFilters {
    lowercase: true,
    remove_punctuation: true,
};
let result = filters.apply("Hello, World!"); // "hello world"
```

### VAD

```rust
use ears::{EnergyVad, VadConfig, VadResult};

let mut vad = EnergyVad::new(VadConfig::default());
let result = vad.process_frame(&samples)?;
match result {
    VadResult::Speech => { /* speaking */ }
    VadResult::Silence => { /* quiet */ }
}
```

### Streaming

```rust
use ears::{LocalAgreementPolicy, StreamingConfig, AudioBuffer};

let mut policy = LocalAgreementPolicy::new(2);
let (committed, uncommitted) = policy.process("Hello world".into());
```

## Error Types

- `WhisperError` - Server connection, transcription, audio file errors
- `StateError` - Invalid transitions, timeout, corruption
- `ProcessError` - Spawn, signal, cleanup errors
- `LockError` - Lock acquisition failures

## Full API Reference

```bash
cargo doc --open
```
