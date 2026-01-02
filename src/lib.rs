//! ears - Speech recognition daemon
//!
//! This library provides core functionality for the ears speech recognition daemon.
//! It handles configuration, state management, audio recording, and Whisper integration.

pub mod config;
pub mod state;
pub mod tui;

pub use config::Config;
pub use state::State;
