//! ears - Production-grade speech recognition daemon for Linux
//!
//! This library provides whisper.cpp integration for speech recognition.

pub mod whisper;

pub use whisper::{WhisperClient, WhisperError};
