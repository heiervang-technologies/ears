//! ears - Production-grade speech recognition daemon for Linux
//!
//! This library provides configuration, state management, process control,
//! and whisper.cpp integration for speech recognition.

// Audio device discovery
pub mod audio;

// Iteration 1: Configuration
pub mod config;

// Iteration 2: State management and process control
pub mod lock;
pub mod process;
pub mod state;

// Iteration 4: Whisper integration
pub mod whisper;

// Iteration 6: Desktop integration
pub mod desktop;

// Iteration 7: TUI
pub mod tui;

// Iteration 8: Streaming transcription with VAD
pub mod continuous_capture;
pub mod progressive_typing;
pub mod streaming;
pub mod streaming_engine;
pub mod vad;

// Iteration 9: Text filters
pub mod text_filters;

// Re-exports from Iteration 1
pub use config::Config;

// Re-exports from Iteration 2
pub use lock::{FileLock, LockError};
pub use process::{ProcessError, ProcessManager};
pub use state::{State, StateError, StateManager};

// Re-exports from Iteration 4
pub use whisper::{WhisperClient, WhisperError};

// Re-exports from Iteration 6
pub use desktop::{AudioFeedback, KeyboardLayout, Notifications, TextInput, TypingMode, Urgency};

// Re-exports from Iteration 8
pub use streaming::{
    AudioBuffer, LocalAgreementPolicy, StreamingConfig, StreamingError, TranscriptChunk,
};
pub use vad::{SileroVad, SpeechSegment, VadConfig, VadError, VadResult, VadSegmentDetector};

// Re-exports from Iteration 9
pub use text_filters::TextFilters;
