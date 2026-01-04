//! Continuous audio capture for streaming transcription
//!
//! This module handles continuous audio recording from PipeWire for VAD mode.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Errors that can occur during continuous audio capture
#[derive(Error, Debug)]
pub enum ContinuousCaptureError {
    #[error("Failed to start audio capture: {0}")]
    StartError(String),

    #[error("Audio capture process died")]
    ProcessDied,

    #[error("Failed to read audio data: {0}")]
    ReadError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Configuration for continuous audio capture
#[derive(Debug, Clone)]
pub struct ContinuousCaptureConfig {
    /// Audio device to capture from
    pub device: String,

    /// Sample rate (Hz)
    pub sample_rate: usize,

    /// Chunk size in samples to read at a time
    pub chunk_size: usize,
}

impl Default for ContinuousCaptureConfig {
    fn default() -> Self {
        Self {
            device: "default".to_string(),
            sample_rate: 16000,
            chunk_size: 1600, // 100ms at 16kHz
        }
    }
}

/// Continuous audio capture handler
pub struct ContinuousCapture {
    /// pw-record child process
    process: Option<Child>,

    /// Configuration
    config: ContinuousCaptureConfig,

    /// Channel for sending audio samples
    audio_tx: Option<mpsc::UnboundedSender<Vec<f32>>>,
}

impl ContinuousCapture {
    /// Create a new ContinuousCapture
    pub fn new(config: ContinuousCaptureConfig, _temp_dir: PathBuf) -> Self {
        Self {
            process: None,
            config,
            audio_tx: None,
        }
    }

    /// Set audio sample sender
    pub fn set_audio_sender(&mut self, tx: mpsc::UnboundedSender<Vec<f32>>) {
        self.audio_tx = Some(tx);
    }

    /// Start continuous audio capture
    pub async fn start(&mut self) -> Result<(), ContinuousCaptureError> {
        if self.process.is_some() {
            warn!("Audio capture already running");
            return Ok(());
        }

        info!(
            "Starting continuous audio capture from device: {}",
            self.config.device
        );

        // Start pw-record in continuous mode
        // Output raw PCM data (16-bit signed, mono, 16kHz)
        let process = Command::new("pw-record")
            .arg("--target")
            .arg(&self.config.device)
            .arg("--rate")
            .arg(self.config.sample_rate.to_string())
            .arg("--channels")
            .arg("1") // Mono
            .arg("--format")
            .arg("s16") // 16-bit signed
            .arg("-") // Output to stdout
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ContinuousCaptureError::StartError(e.to_string()))?;

        self.process = Some(process);

        // Spawn background task to read audio
        self.spawn_reader_task();

        Ok(())
    }

    /// Stop continuous audio capture
    pub fn stop(&mut self) -> Result<(), ContinuousCaptureError> {
        if let Some(mut process) = self.process.take() {
            info!("Stopping continuous audio capture");

            // Kill the process
            let _ = process.kill();

            // Wait for it to exit
            let _ = process.wait();
        }

        Ok(())
    }

    /// Spawn background task to read audio samples
    fn spawn_reader_task(&mut self) {
        let mut process = match self.process.take() {
            Some(p) => p,
            None => return,
        };

        let audio_tx = match self.audio_tx.clone() {
            Some(tx) => tx,
            None => {
                warn!("No audio sender set, cannot read samples");
                return;
            }
        };

        let chunk_size = self.config.chunk_size;

        // Spawn task to read from stdout
        tokio::spawn(async move {
            let mut stdout = match process.stdout.take() {
                Some(stdout) => stdout,
                None => {
                    warn!("Failed to get stdout from pw-record");
                    return;
                }
            };

            // Buffer for reading raw PCM data (16-bit samples)
            let mut buffer = vec![0u8; chunk_size * 2]; // 2 bytes per sample

            loop {
                match stdout.read_exact(&mut buffer) {
                    Ok(_) => {
                        // Convert i16 samples to f32
                        let samples: Vec<f32> = buffer
                            .chunks_exact(2)
                            .map(|bytes| {
                                let sample_i16 = i16::from_le_bytes([bytes[0], bytes[1]]);
                                sample_i16 as f32 / 32768.0 // Normalize to -1.0..1.0
                            })
                            .collect();

                        // Send samples
                        if audio_tx.send(samples).is_err() {
                            debug!("Audio receiver dropped, stopping capture");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read audio: {}", e);
                        break;
                    }
                }
            }

            // Clean up process
            let _ = process.kill();
            let _ = process.wait();

            debug!("Audio capture reader task ended");
        });
    }

    /// Check if capture is running
    pub fn is_running(&self) -> bool {
        self.process.is_some()
    }
}

impl Drop for ContinuousCapture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_continuous_capture_config_default() {
        let config = ContinuousCaptureConfig::default();
        assert_eq!(config.device, "default");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.chunk_size, 1600);
    }

    #[test]
    fn test_continuous_capture_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = ContinuousCaptureConfig::default();
        let capture = ContinuousCapture::new(config, temp_dir.path().to_path_buf());

        assert!(!capture.is_running());
    }
}
