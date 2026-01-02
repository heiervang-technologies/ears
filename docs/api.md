# API Documentation

This document covers the `ears` library API for embedding speech recognition into your own Rust applications.

## Table of Contents

- [Overview](#overview)
- [Getting Started](#getting-started)
- [Core Modules](#core-modules)
  - [Config](#config)
  - [State Management](#state-management)
  - [Process Control](#process-control)
  - [Whisper Integration](#whisper-integration)
  - [Desktop Integration](#desktop-integration)
- [Examples](#examples)
- [Error Handling](#error-handling)

## Overview

The `ears` library provides production-ready components for building speech recognition applications on Linux:

- **Configuration management** - Load/save server URLs, devices, paths
- **State management** - Track recording/transcription states with validation
- **Process control** - Manage audio recording subprocesses safely
- **Whisper integration** - HTTP client for whisper.cpp servers
- **Desktop integration** - Audio feedback, notifications, text input

## Getting Started

### Adding ears to Your Project

Add to `Cargo.toml`:

```toml
[dependencies]
ears = "0.1"
tokio = { version = "1", features = ["full"] }
anyhow = "1.0"
```

### Basic Example

```rust
use ears::{Config, WhisperClient};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration
    let config = Config::load()?;
    
    // Create whisper client
    let client = WhisperClient::new(config.whisper_server.clone())?;
    
    // Check if server is healthy
    client.health_check().await?;
    
    println!("Connected to whisper.cpp at {}", config.whisper_server);
    
    Ok(())
}
```

## Core Modules

### Config

Configuration management for server URLs, audio devices, and directories.

#### Types

```rust
pub struct Config {
    pub whisper_server: Url,
    pub device: String,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
}
```

#### Creating Configuration

```rust
use ears::Config;
use url::Url;

// Create with defaults
let config = Config::new()?;

// Create with custom values
let config = Config {
    whisper_server: Url::parse("http://localhost:8178")?,
    device: "alsa_input.usb-Blue_Microphones_Yeti".to_string(),
    config_dir: dirs::config_dir().unwrap().join("ears"),
    state_dir: dirs::runtime_dir().unwrap().join("ears"),
};
```

#### Loading from Files

```rust
use ears::Config;

// Load from ~/.config/ears/{server,device}
let config = Config::load()?;

// Load with environment variable overrides
let config = Config::from_env()?;
```

Environment variables:
- `EARS_SERVER` - Override whisper server URL
- `EARS_DEVICE` - Override audio device

#### Saving Configuration

```rust
use ears::Config;

let mut config = Config::load()?;
config.whisper_server = Url::parse("http://192.168.1.100:8178")?;
config.save()?;

// Now ~/.config/ears/server contains the new URL
```

#### Validation

```rust
use ears::Config;

let config = Config::load()?;
config.validate()?;  // Checks URL scheme is http/https
```

### State Management

Track application state with validated transitions.

#### State Enum

```rust
pub enum State {
    Idle,
    Recording { since: SystemTime },
    Transcribing,
}
```

#### StateManager

```rust
use ears::{State, StateManager};

let state_dir = "/tmp/ears";
let mut manager = StateManager::new(state_dir)?;

// Get current state
let state = manager.current_state()?;
match state {
    State::Idle => println!("Not recording"),
    State::Recording { since } => println!("Recording since {:?}", since),
    State::Transcribing => println!("Transcribing..."),
}

// Transition to Recording
manager.transition_to(State::Recording { 
    since: SystemTime::now() 
})?;

// Transition to Transcribing
manager.transition_to(State::Transcribing)?;

// Back to Idle
manager.transition_to(State::Idle)?;
```

#### Valid State Transitions

| From | To | Valid? |
|------|-----|--------|
| Idle | Recording | ✅ |
| Idle | Transcribing | ❌ |
| Recording | Transcribing | ✅ |
| Recording | Idle | ✅ (emergency stop) |
| Transcribing | Idle | ✅ |
| Transcribing | Recording | ❌ |

#### Recording Timeout

```rust
use ears::{State, StateManager};
use std::time::Duration;

let mut manager = StateManager::new("/tmp/ears")?;

// Check if recording has exceeded timeout (2 minutes)
if manager.recording_timeout_exceeded(Duration::from_secs(120))? {
    println!("Recording timed out!");
    manager.transition_to(State::Idle)?;
}
```

### Process Control

Manage audio recording subprocesses with PID tracking and graceful shutdown.

#### Lock Files

```rust
use ears::FileLock;

// Try to acquire lock (non-blocking)
match FileLock::try_lock("/tmp/ears/lock")? {
    Some(lock) => {
        println!("Lock acquired");
        // Lock is automatically released when `lock` is dropped
    }
    None => {
        println!("Already locked");
    }
}

// Blocking lock acquisition
let lock = FileLock::lock("/tmp/ears/lock")?;
// ... do work ...
// Lock released when `lock` goes out of scope
```

#### Process Management

```rust
use ears::ProcessManager;
use std::process::Command;

let state_dir = "/tmp/ears";
let manager = ProcessManager::new(state_dir);

// Start a recording process
let mut command = Command::new("pw-record");
command.arg("--target=alsa_input.usb-...");
command.arg("/tmp/recording.wav");

let pid = manager.spawn(command)?;
println!("Recording with PID {}", pid);

// Check if process is still running
if manager.is_process_running()? {
    println!("Process is active");
}

// Stop the process gracefully (SIGTERM, then SIGKILL if needed)
manager.stop_process()?;

// Clean up PID file
manager.cleanup()?;
```

#### PID File Management

```rust
use ears::ProcessManager;

let manager = ProcessManager::new("/tmp/ears");

// PID file is at /tmp/ears/recording.pid
if let Some(pid) = manager.read_pid()? {
    println!("Recording process: {}", pid);
}

// Check for stale PIDs (process not actually running)
if manager.has_stale_pid()? {
    println!("Found stale PID file, cleaning up");
    manager.cleanup()?;
}
```

### Whisper Integration

HTTP client for whisper.cpp servers with retry logic and error handling.

#### WhisperClient

```rust
use ears::WhisperClient;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let server_url = Url::parse("http://localhost:8178")?;
    let client = WhisperClient::new(server_url)?;
    
    // Health check
    match client.health_check().await {
        Ok(()) => println!("Server is healthy"),
        Err(e) => eprintln!("Server unhealthy: {}", e),
    }
    
    Ok(())
}
```

#### Transcription

```rust
use ears::WhisperClient;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<()> {
    let client = WhisperClient::new(
        Url::parse("http://localhost:8178")?
    )?;
    
    let audio_file = Path::new("/tmp/recording.wav");
    
    // Transcribe with default retry (3 attempts)
    let text = client.transcribe(audio_file).await?;
    println!("Transcription: {}", text);
    
    Ok(())
}
```

#### Custom Retry Configuration

```rust
use ears::WhisperClient;
use std::time::Duration;

let mut client = WhisperClient::new(
    Url::parse("http://localhost:8178")?
)?;

// Configure retry behavior
client.set_max_retries(5);
client.set_retry_delay(Duration::from_secs(2));

// Now transcribe with custom retry settings
let text = client.transcribe(audio_file).await?;
```

#### Silence Artifact Filtering

The client automatically filters common whisper.cpp silence artifacts:

```rust
// These are filtered out:
// - Empty strings
// - "Thank you."
// - "Thanks for watching."

let text = client.transcribe(silent_audio).await?;
// Returns None if only silence artifacts detected
if let Some(text) = text {
    println!("Real transcription: {}", text);
} else {
    println!("Only silence detected");
}
```

#### Error Handling

```rust
use ears::{WhisperClient, WhisperError};

match client.transcribe(audio_file).await {
    Ok(Some(text)) => println!("Success: {}", text),
    Ok(None) => println!("Silence detected"),
    Err(WhisperError::ServerNotAvailable) => {
        eprintln!("Server is down or unreachable");
    }
    Err(WhisperError::TranscriptionFailed(msg)) => {
        eprintln!("Transcription error: {}", msg);
    }
    Err(WhisperError::InvalidAudioFile(msg)) => {
        eprintln!("Bad audio file: {}", msg);
    }
    Err(e) => eprintln!("Other error: {}", e),
}
```

### Desktop Integration

Audio feedback, desktop notifications, and text input automation.

#### Audio Feedback

```rust
use ears::AudioFeedback;

let feedback = AudioFeedback::new();

// Play start sound (blocks until complete)
feedback.play_start();

// Play completion sound
feedback.play_done();

// Play error sound
feedback.play_error();
```

Custom sounds are loaded from `~/.local/share/ears-sounds/`:
- `start.wav` - Recording started
- `done.wav` - Transcription complete
- `bell.wav` - Error occurred

Falls back to system sounds if custom sounds aren't found.

#### Desktop Notifications

```rust
use ears::{Notifications, Urgency};

let notifications = Notifications::new();

// Simple notification
notifications.show(
    "ears",
    "Recording started",
    Urgency::Normal
);

// Error notification (critical urgency)
notifications.error("Failed to connect to whisper.cpp server");

// Info notification (low urgency)
notifications.info("Transcription complete");
```

Urgency levels:
- `Urgency::Low` - Non-critical info
- `Urgency::Normal` - Standard notifications
- `Urgency::Critical` - Errors, important alerts

#### Text Input

```rust
use ears::TextInput;

let input = TextInput::new();

// Type text at cursor position
input.type_text("Hello, world!");

// Type with custom delay between characters (default: 12ms)
input.type_text_with_delay("Slower typing", 50);
```

Requires `ydotool` daemon to be running:
```bash
ydotoold &
```

## Examples

### Complete Recording and Transcription Flow

```rust
use ears::{
    Config, StateManager, ProcessManager, WhisperClient,
    State, AudioFeedback, Notifications, TextInput
};
use std::process::Command;
use std::time::{Duration, SystemTime};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Load config
    let config = Config::load()?;
    
    // Initialize components
    let mut state_mgr = StateManager::new(&config.state_dir)?;
    let proc_mgr = ProcessManager::new(&config.state_dir);
    let whisper = WhisperClient::new(config.whisper_server.clone())?;
    let feedback = AudioFeedback::new();
    let notifications = Notifications::new();
    let input = TextInput::new();
    
    // Check whisper server health
    whisper.health_check().await?;
    
    // Get current state
    let current = state_mgr.current_state()?;
    
    match current {
        State::Idle => {
            // Start recording
            println!("Starting recording...");
            
            let audio_file = config.state_dir.join("recording.wav");
            let mut cmd = Command::new("pw-record");
            cmd.arg(format!("--target={}", config.device));
            cmd.arg(&audio_file);
            
            proc_mgr.spawn(cmd)?;
            state_mgr.transition_to(State::Recording {
                since: SystemTime::now()
            })?;
            
            feedback.play_start();
            notifications.info("Recording started");
        }
        
        State::Recording { since } => {
            // Check timeout
            let timeout = Duration::from_secs(120);
            if state_mgr.recording_timeout_exceeded(timeout)? {
                notifications.error("Recording timed out");
                proc_mgr.stop_process()?;
                state_mgr.transition_to(State::Idle)?;
                return Ok(());
            }
            
            // Stop recording and transcribe
            println!("Stopping recording...");
            proc_mgr.stop_process()?;
            state_mgr.transition_to(State::Transcribing)?;
            
            // Wait for file to be written
            tokio::time::sleep(Duration::from_millis(300)).await;
            
            let audio_file = config.state_dir.join("recording.wav");
            
            // Transcribe
            match whisper.transcribe(&audio_file).await? {
                Some(text) => {
                    println!("Transcription: {}", text);
                    input.type_text(&text);
                    feedback.play_done();
                    notifications.info("Transcription complete");
                }
                None => {
                    println!("Silence detected");
                    notifications.info("No speech detected");
                }
            }
            
            // Clean up
            std::fs::remove_file(&audio_file).ok();
            state_mgr.transition_to(State::Idle)?;
        }
        
        State::Transcribing => {
            notifications.error("Already transcribing, please wait");
        }
    }
    
    Ok(())
}
```

### Simple Transcription Service

```rust
use ears::{Config, WhisperClient};
use std::path::PathBuf;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let client = WhisperClient::new(config.whisper_server)?;
    
    // Process multiple files
    let files = vec![
        PathBuf::from("recording1.wav"),
        PathBuf::from("recording2.wav"),
        PathBuf::from("recording3.wav"),
    ];
    
    for file in files {
        match client.transcribe(&file).await {
            Ok(Some(text)) => {
                let output = file.with_extension("txt");
                std::fs::write(output, text)?;
                println!("Processed: {}", file.display());
            }
            Ok(None) => {
                println!("Silence in: {}", file.display());
            }
            Err(e) => {
                eprintln!("Failed {}: {}", file.display(), e);
            }
        }
    }
    
    Ok(())
}
```

### Custom State Machine

```rust
use ears::{State, StateManager};
use std::time::{Duration, SystemTime};
use anyhow::Result;

fn main() -> Result<()> {
    let mut mgr = StateManager::new("/tmp/my-app")?;
    
    // Idle -> Recording
    mgr.transition_to(State::Recording {
        since: SystemTime::now()
    })?;
    
    // Simulate recording for 10 seconds
    std::thread::sleep(Duration::from_secs(10));
    
    // Check timeout (30 second max)
    if mgr.recording_timeout_exceeded(Duration::from_secs(30))? {
        println!("Would have timed out!");
    } else {
        // Recording -> Transcribing
        mgr.transition_to(State::Transcribing)?;
        
        // Simulate transcription
        std::thread::sleep(Duration::from_secs(2));
        
        // Transcribing -> Idle
        mgr.transition_to(State::Idle)?;
    }
    
    Ok(())
}
```

### Async Recording Manager

```rust
use ears::{ProcessManager, WhisperClient, Config};
use tokio::process::Command;
use std::path::PathBuf;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load()?;
    let proc_mgr = ProcessManager::new(&config.state_dir);
    let whisper = WhisperClient::new(config.whisper_server)?;
    
    let audio_file = config.state_dir.join("recording.wav");
    
    // Start recording (async)
    let mut cmd = tokio::process::Command::new("pw-record");
    cmd.arg(format!("--target={}", config.device));
    cmd.arg(&audio_file);
    
    let child = cmd.spawn()?;
    let pid = child.id().unwrap();
    println!("Recording with PID {}", pid);
    
    // Record for 5 seconds
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    
    // Stop recording (send SIGTERM via sync API)
    proc_mgr.stop_process()?;
    
    // Wait a bit for file to be written
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Transcribe
    if let Some(text) = whisper.transcribe(&audio_file).await? {
        println!("Transcription: {}", text);
    }
    
    Ok(())
}
```

## Error Handling

### Error Types

Each module has its own error type:

```rust
use ears::{WhisperError, StateError, ProcessError, LockError};

// Whisper errors
pub enum WhisperError {
    ServerNotAvailable,
    TranscriptionFailed(String),
    InvalidAudioFile(String),
    NetworkError(String),
}

// State errors
pub enum StateError {
    InvalidTransition { from: State, to: State },
    StateFileCorrupted,
    IoError(std::io::Error),
}

// Process errors
pub enum ProcessError {
    SpawnFailed(std::io::Error),
    ProcessNotRunning,
    SignalFailed,
}

// Lock errors
pub enum LockError {
    LockFailed,
    AlreadyLocked,
    IoError(std::io::Error),
}
```

### Error Handling Patterns

#### Using anyhow for Application Code

```rust
use anyhow::{Result, Context};

fn record_and_transcribe() -> Result<()> {
    let config = Config::load()
        .context("Failed to load configuration")?;
    
    let client = WhisperClient::new(config.whisper_server)
        .context("Failed to create whisper client")?;
    
    // ... more code
    
    Ok(())
}
```

#### Pattern Matching for Specific Errors

```rust
use ears::WhisperError;

match client.transcribe(audio).await {
    Ok(Some(text)) => { /* handle success */ }
    Ok(None) => { /* handle silence */ }
    Err(WhisperError::ServerNotAvailable) => {
        // Specific handling for server down
        reconnect_or_notify()?;
    }
    Err(e) => {
        // Generic error handling
        log::error!("Transcription failed: {}", e);
    }
}
```

#### Retry Logic

```rust
use ears::WhisperClient;
use std::time::Duration;

async fn transcribe_with_retry(
    client: &WhisperClient,
    audio: &Path,
    max_attempts: u32
) -> Result<Option<String>> {
    for attempt in 1..=max_attempts {
        match client.transcribe(audio).await {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_attempts => {
                log::warn!("Attempt {} failed: {}", attempt, e);
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}
```

## Best Practices

1. **Always validate configuration** before use:
   ```rust
   let config = Config::load()?;
   config.validate()?;
   ```

2. **Use state transitions** for safety:
   ```rust
   // StateManager prevents invalid transitions
   manager.transition_to(new_state)?;  // Returns error if invalid
   ```

3. **Lock files prevent race conditions**:
   ```rust
   let _lock = FileLock::lock(path)?;  // Blocks until available
   // Automatic release on drop
   ```

4. **Clean up resources** properly:
   ```rust
   proc_mgr.stop_process()?;
   proc_mgr.cleanup()?;
   std::fs::remove_file(audio_file).ok();
   ```

5. **Check whisper server health** before recording:
   ```rust
   if let Err(e) = client.health_check().await {
       return Err(anyhow!("Server unavailable: {}", e));
   }
   ```

6. **Handle silence artifacts**:
   ```rust
   match client.transcribe(audio).await? {
       Some(text) => use_transcription(text),
       None => println!("Silence detected, ignoring"),
   }
   ```

## Thread Safety

- **Config:** `Send` + `Sync` safe (immutable after creation)
- **StateManager:** Not `Sync` (use from single thread or wrap in `Mutex`)
- **ProcessManager:** Not `Sync` (use from single thread or wrap in `Mutex`)
- **WhisperClient:** `Send` + `Sync` safe (can share across threads)
- **FileLock:** `Send` but not `Sync` (owns the lock)

For multi-threaded usage:

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

let whisper = Arc::new(WhisperClient::new(url)?);
let state = Arc::new(Mutex::new(StateManager::new(dir)?));

// Clone for use in different tasks
let whisper_clone = Arc::clone(&whisper);
let state_clone = Arc::clone(&state);

tokio::spawn(async move {
    let text = whisper_clone.transcribe(audio).await?;
    let mut s = state_clone.lock().await;
    s.transition_to(State::Idle)?;
    Ok::<_, anyhow::Error>(())
});
```

## Next Steps

- **User Guide:** See [user-guide.md](user-guide.md) for end-user documentation
- **Architecture:** See [architecture.md](architecture.md) for implementation details
- **Examples:** Check the `examples/` directory in the repository
- **Tests:** See `tests/` for more usage examples

## Reference Documentation

Full API documentation is available via `cargo doc`:

```bash
cargo doc --open
```

This will open detailed API docs with all public types, traits, and functions.
