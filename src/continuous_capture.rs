//! Continuous audio capture for streaming transcription
//!
//! This module handles continuous audio recording from PipeWire for VAD mode.
//!
//! ## Ownership and liveness
//!
//! The `pw-record` child is shared between the capture handle and the reader
//! task, so `stop()` (and `Drop`) can always kill and reap it even while the
//! reader is blocked in `read`. The reader publishes its liveness through a
//! [`CaptureStatus`] watch channel; consumers select on [`ContinuousCapture::status_rx`]
//! to learn that capture died (device unplugged, PipeWire restart, EOF) rather
//! than waiting on an audio channel that will never close.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::{mpsc, watch};
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

/// Liveness of the capture pipeline as observed by the reader task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStatus {
    /// Not started, or stopped by request and ready to start again.
    Idle,
    /// pw-record is running and the reader is delivering chunks.
    Running,
    /// Capture ended on its own. `reason` is human-readable and stable
    /// enough to log or show; it is not a machine-parsed contract.
    Stopped { reason: String },
}

impl CaptureStatus {
    /// Convenience: is the reader alive and delivering audio?
    pub fn is_running(&self) -> bool {
        matches!(self, CaptureStatus::Running)
    }
}

/// Shared handle to the pw-record child, tagged with the start generation
/// that spawned it so a reader from an earlier `start()` can never kill or
/// report on a child that belongs to a later one.
type SharedChild = Arc<Mutex<Option<(u64, Child)>>>;

/// Continuous audio capture handler
pub struct ContinuousCapture {
    /// pw-record child process, shared with the reader task
    process: SharedChild,

    /// Incremented on every `start()`; readers act only on their own generation.
    generation: Arc<AtomicU64>,
    health: Option<crate::health::PipelineHealth>,

    /// Configuration
    config: ContinuousCaptureConfig,

    /// Channel for sending audio samples
    audio_tx: Option<mpsc::UnboundedSender<Vec<f32>>>,

    /// Liveness publisher (reader task writes, owners subscribe)
    status_tx: watch::Sender<CaptureStatus>,

    /// Cached receiver so `status_rx()` is cheap to call
    status_rx: watch::Receiver<CaptureStatus>,
}

impl ContinuousCapture {
    /// Create a new ContinuousCapture
    pub fn new(config: ContinuousCaptureConfig, _temp_dir: PathBuf) -> Self {
        let (status_tx, status_rx) = watch::channel(CaptureStatus::Idle);
        Self {
            process: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            health: None,
            config,
            audio_tx: None,
            status_tx,
            status_rx,
        }
    }

    /// Set audio sample sender
    pub fn set_audio_sender(&mut self, tx: mpsc::UnboundedSender<Vec<f32>>) {
        self.audio_tx = Some(tx);
    }

    /// Subscribe to capture liveness. The receiver flips to
    /// `CaptureStatus::Stopped { .. }` when the reader exits for any reason
    /// other than an explicit `stop()`.
    pub fn status_rx(&self) -> watch::Receiver<CaptureStatus> {
        self.status_rx.clone()
    }

    /// Current liveness snapshot.
    pub fn status(&self) -> CaptureStatus {
        self.status_rx.borrow().clone()
    }

    pub fn set_health(&mut self, health: crate::health::PipelineHealth) {
        self.health = Some(health);
    }

    /// Start continuous audio capture
    pub async fn start(&mut self) -> Result<(), ContinuousCaptureError> {
        if self.is_running() {
            warn!("Audio capture already running");
            return Ok(());
        }

        let audio_tx = self
            .audio_tx
            .clone()
            .ok_or_else(|| ContinuousCaptureError::StartError("no audio sender set".to_string()))?;

        info!(
            "Starting continuous audio capture from device: {}",
            self.config.device
        );

        // Start pw-record in continuous mode
        // Output raw PCM data (16-bit signed, mono, 16kHz)
        let mut child = Command::new("pw-record")
            .arg("--target")
            .arg(&self.config.device)
            .arg("--rate")
            .arg(self.config.sample_rate.to_string())
            .arg("--channels")
            .arg("1") // Mono
            .arg("--format")
            .arg("s16") // 16-bit signed
            .arg("-") // Output to stdout
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ContinuousCaptureError::StartError(e.to_string()))?;

        // Take stdout before sharing the child, so the reader owns the pipe
        // and the shared slot only needs to be locked for kill/reap.
        let stdout = child.stdout.take().ok_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            ContinuousCaptureError::StartError("pw-record stdout unavailable".to_string())
        })?;

        // Make sure no previous child lingers, then claim a new generation.
        Self::kill_and_reap(&self.process);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut slot) = self.process.lock() {
            *slot = Some((generation, child));
        }
        let _ = self.status_tx.send(CaptureStatus::Running);

        self.spawn_reader_task(stdout, audio_tx, generation);

        Ok(())
    }

    /// Stop continuous audio capture: kill and reap pw-record.
    ///
    /// The reader task observes EOF on the pipe and exits; because the stop
    /// was requested, the status becomes `Idle` rather than `Stopped`.
    pub fn stop(&mut self) -> Result<(), ContinuousCaptureError> {
        // Announce the intent first so the reader's exit is not misreported.
        if self.status().is_running() {
            info!("Stopping continuous audio capture");
            let _ = self.status_tx.send(CaptureStatus::Idle);
        }
        Self::kill_and_reap(&self.process);
        Ok(())
    }

    fn kill_and_reap(process: &SharedChild) {
        let child = match process.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => None,
        };
        if let Some((_, mut child)) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Spawn background task to read audio samples
    fn spawn_reader_task(
        &mut self,
        mut stdout: std::process::ChildStdout,
        audio_tx: mpsc::UnboundedSender<Vec<f32>>,
        generation: u64,
    ) {
        let chunk_size = self.config.chunk_size;
        let process = self.process.clone();
        let status_tx = self.status_tx.clone();
        let current_generation = self.generation.clone();
        let health = self.health.clone();

        // Use spawn_blocking for the reader since it does blocking std::io::Read.
        // tokio::spawn with blocking I/O would starve the async runtime.
        tokio::task::spawn_blocking(move || {
            // Buffer for reading raw PCM data (16-bit samples)
            let mut buffer = vec![0u8; chunk_size * 2]; // 2 bytes per sample

            let exit_reason: Option<String> = loop {
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

                        if let Some(ref health) = health {
                            health.captured(&samples);
                        }

                        // Send samples
                        if audio_tx.send(samples).is_err() {
                            debug!("Audio receiver dropped, stopping capture");
                            break None;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        break Some("pw-record closed its output (EOF)".to_string());
                    }
                    Err(e) => {
                        break Some(format!("failed to read audio: {}", e));
                    }
                }
            };

            // Reap *our* child and enrich the reason with its exit status.
            // If the slot now holds a later generation's child, leave it alone.
            let exit_status = match process.lock() {
                Ok(mut slot) => match slot.take() {
                    Some((gen, mut child)) if gen == generation => {
                        let _ = child.kill();
                        child.wait().ok()
                    }
                    Some(other) => {
                        *slot = Some(other);
                        None
                    }
                    None => None,
                },
                Err(_) => None,
            };

            // A newer start() owns the status now; this reader is history.
            if current_generation.load(Ordering::SeqCst) != generation {
                debug!(
                    "Audio capture reader (gen {}) ended after a restart",
                    generation
                );
                return;
            }

            // Only publish Stopped if nobody asked us to stop. `stop()` sets
            // Idle before killing the child, so an EOF after that is expected.
            let requested = !status_tx.borrow().is_running();
            match exit_reason {
                Some(reason) if !requested => {
                    let reason = match exit_status {
                        Some(status) => format!("{} (pw-record exit: {})", reason, status),
                        None => reason,
                    };
                    if let Some(ref health) = health {
                        health.capture_stopped(&reason);
                    }
                    warn!("Audio capture stopped: {}", reason);
                    let _ = status_tx.send(CaptureStatus::Stopped { reason });
                }
                _ => {
                    let _ = status_tx.send(CaptureStatus::Idle);
                }
            }

            debug!("Audio capture reader task ended");
        });
    }

    /// Check if capture is running (reader alive and delivering audio)
    pub fn is_running(&self) -> bool {
        self.status().is_running()
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
        assert_eq!(capture.status(), CaptureStatus::Idle);
    }

    #[tokio::test]
    async fn test_start_without_sender_fails() {
        let temp_dir = TempDir::new().unwrap();
        let mut capture =
            ContinuousCapture::new(ContinuousCaptureConfig::default(), temp_dir.path().into());
        let err = capture.start().await.unwrap_err();
        assert!(matches!(err, ContinuousCaptureError::StartError(_)));
        assert!(!capture.is_running());
    }

    /// Drive the reader against a fake producer child (`head -c`) so the
    /// liveness contract can be checked without PipeWire.
    fn spawn_fake_child(bytes: usize) -> Child {
        Command::new("sh")
            .arg("-c")
            .arg(format!("head -c {} /dev/zero", bytes))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh available")
    }

    #[tokio::test]
    async fn test_reader_eof_publishes_stopped() {
        let temp_dir = TempDir::new().unwrap();
        let config = ContinuousCaptureConfig {
            chunk_size: 100,
            ..ContinuousCaptureConfig::default()
        };
        let mut capture = ContinuousCapture::new(config, temp_dir.path().into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        capture.set_audio_sender(tx);

        // Two full chunks, then EOF.
        let mut child = spawn_fake_child(400);
        let stdout = child.stdout.take().unwrap();
        let generation = capture.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *capture.process.lock().unwrap() = Some((generation, child));
        let _ = capture.status_tx.send(CaptureStatus::Running);
        let audio_tx = capture.audio_tx.clone().unwrap();
        capture.spawn_reader_task(stdout, audio_tx, generation);

        let mut status_rx = capture.status_rx();
        assert_eq!(rx.recv().await.unwrap().len(), 100);
        assert_eq!(rx.recv().await.unwrap().len(), 100);

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                status_rx.changed().await.unwrap();
                if let CaptureStatus::Stopped { reason } = &*status_rx.borrow() {
                    assert!(reason.contains("EOF"), "reason: {}", reason);
                    break;
                }
            }
        })
        .await
        .expect("reader must publish Stopped on EOF");

        assert!(!capture.is_running());
        assert!(capture.process.lock().unwrap().is_none(), "child reaped");
    }

    /// Install a fake child as if `start()` had spawned it.
    fn install_fake(capture: &mut ContinuousCapture, mut child: Child) -> u64 {
        let stdout = child.stdout.take().unwrap();
        let generation = capture.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *capture.process.lock().unwrap() = Some((generation, child));
        let _ = capture.status_tx.send(CaptureStatus::Running);
        let audio_tx = capture.audio_tx.clone().unwrap();
        capture.spawn_reader_task(stdout, audio_tx, generation);
        generation
    }

    fn sh(script: &str) -> Child {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn test_old_reader_does_not_touch_restarted_capture() {
        let temp_dir = TempDir::new().unwrap();
        let config = ContinuousCaptureConfig {
            chunk_size: 100,
            ..ContinuousCaptureConfig::default()
        };
        let mut capture = ContinuousCapture::new(config, temp_dir.path().into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        capture.set_audio_sender(tx);

        // Generation 1: writes one chunk, then holds the pipe open a while
        // so its reader is still alive when we restart.
        let gen1 = install_fake(&mut capture, sh("head -c 200 /dev/zero; sleep 0.3"));
        assert!(rx.recv().await.is_some());

        // "Restart": stop() then install generation 2 (an endless producer).
        capture.stop().unwrap();
        let gen2 = install_fake(&mut capture, sh("cat /dev/zero"));
        assert_ne!(gen1, gen2);
        assert!(capture.is_running());

        // Let generation 1's reader hit EOF and exit.
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;

        // Generation 2 must be untouched: still Running, child still in slot.
        assert_eq!(capture.status(), CaptureStatus::Running);
        {
            let slot = capture.process.lock().unwrap();
            assert!(
                matches!(&*slot, Some((g, _)) if *g == gen2),
                "gen2 child must remain in the slot"
            );
        }
        assert!(rx.recv().await.is_some(), "gen2 still delivering audio");

        capture.stop().unwrap();
    }

    #[tokio::test]
    async fn test_stop_reports_idle_not_stopped() {
        let temp_dir = TempDir::new().unwrap();
        let config = ContinuousCaptureConfig {
            chunk_size: 100,
            ..ContinuousCaptureConfig::default()
        };
        let mut capture = ContinuousCapture::new(config, temp_dir.path().into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        capture.set_audio_sender(tx);

        // A producer that never ends on its own.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("cat /dev/zero")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let generation = capture.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *capture.process.lock().unwrap() = Some((generation, child));
        let _ = capture.status_tx.send(CaptureStatus::Running);
        let audio_tx = capture.audio_tx.clone().unwrap();
        capture.spawn_reader_task(stdout, audio_tx, generation);

        assert!(rx.recv().await.is_some());
        assert!(capture.is_running());

        let mut status_rx = capture.status_rx();
        capture.stop().unwrap();
        assert!(!capture.is_running());

        // Give the reader time to notice EOF; it must settle on Idle.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while status_rx.changed().await.is_ok() {
                if !matches!(&*status_rx.borrow(), CaptureStatus::Running) {
                    break;
                }
            }
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(capture.status(), CaptureStatus::Idle);
    }
}
