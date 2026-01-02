//! ears - Production-grade speech recognition daemon for Linux
//!
//! This library provides state management, process control, and whisper.cpp integration.

// Iteration 2: State Management & Process Control
pub mod lock;
pub mod process;
pub mod state;

// Iteration 4: Whisper Integration
pub mod whisper;

pub use lock::{FileLock, LockError};
pub use process::{ProcessError, ProcessManager};
pub use state::{State, StateError, StateManager};
pub use whisper::{WhisperClient, WhisperError};
