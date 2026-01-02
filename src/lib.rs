//! ears - Production-grade speech recognition daemon for Linux
//!
//! This library provides state management, process control, and whisper.cpp integration
//! for speech recognition.

// Iteration 2: State management and process control
pub mod lock;
pub mod process;
pub mod state;
pub mod tui;

// Iteration 4: Whisper integration
pub mod whisper;

// Re-exports from Iteration 2
pub use lock::{FileLock, LockError};
pub use process::{ProcessError, ProcessManager};
pub use state::{State, StateError, StateManager};

// Re-exports from Iteration 4
pub use whisper::{WhisperClient, WhisperError};
