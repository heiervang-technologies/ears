//! ears - Speech recognition daemon
//!
//! This library provides core functionality for the ears speech recognition daemon.
//! It handles configuration, state management, audio recording, and Whisper integration.

pub mod audio;
pub mod config;
pub mod desktop;
pub mod state;
pub mod whisper;

pub use audio::{AudioDevice, DeviceManager, Recorder};
pub use config::Config;
pub use desktop::{AudioFeedback, Notifications, TextInput, Urgency};
pub use state::State;
pub use whisper::WhisperClient;
