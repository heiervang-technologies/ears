//! ears - Production-grade speech recognition daemon for Linux
//!
//! This library provides configuration, state management, process control,
//! and whisper.cpp integration for speech recognition.

// Iteration 1: Configuration
pub mod config;

// Iteration 2: State management and process control
pub mod lock;
pub mod process;
pub mod state;

// Iteration 4: Whisper integration
pub mod whisper;

// Iteration 7: TUI
pub mod tui;

// Re-exports from Iteration 1
pub use config::Config;

// Re-exports from Iteration 2
pub use lock::{FileLock, LockError};
pub use process::{ProcessError, ProcessManager};
pub use state::{State, StateError, StateManager};

// Re-exports from Iteration 4
pub use whisper::{WhisperClient, WhisperError};
