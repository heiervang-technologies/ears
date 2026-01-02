use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// Audio recording format configuration
#[derive(Debug, Clone)]
pub struct RecordingConfig {
    /// Sample rate in Hz (whisper.cpp prefers 16kHz)
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u8,
    /// Audio format (e.g., "s16" for signed 16-bit PCM)
    pub format: String,
    /// Maximum recording duration in seconds
    pub timeout_secs: u32,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            format: "s16".to_string(),
            timeout_secs: 120,
        }
    }
}

/// Represents an active recording session
pub struct Recording {
    /// The child process running pw-record
    process: Child,
    /// Path to the output audio file
    pub output_file: PathBuf,
}

impl Recording {
    /// Start a new recording using PipeWire
    ///
    /// # Arguments
    /// * `device` - The target audio device name (from pw-cli)
    /// * `output_file` - Path where the recording will be saved
    /// * `config` - Recording configuration (sample rate, channels, etc.)
    pub fn start(
        device: &str,
        output_file: PathBuf,
        config: RecordingConfig,
    ) -> Result<Self> {
        // Build the pw-record command
        let mut cmd = Command::new("timeout");
        cmd.arg(config.timeout_secs.to_string())
            .arg("pw-record")
            .arg("--target")
            .arg(device)
            .arg("--rate")
            .arg(config.sample_rate.to_string())
            .arg("--channels")
            .arg(config.channels.to_string())
            .arg("--format")
            .arg(&config.format)
            .arg(&output_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let process = cmd
            .spawn()
            .context("Failed to start recording process")?;

        Ok(Self {
            process,
            output_file,
        })
    }

    /// Get the process ID of the recording
    pub fn pid(&self) -> u32 {
        self.process.id()
    }

    /// Stop the recording and wait for the process to finish
    ///
    /// Returns Ok(()) if the recording was stopped successfully
    pub fn stop(mut self) -> Result<()> {
        // Try to kill the process gracefully
        #[cfg(unix)]
        {
            let pid = self.process.id() as i32;
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }

        // Wait for the process to finish
        let status = self
            .process
            .wait()
            .context("Failed to wait for recording process")?;

        // SIGTERM results in exit code 143, which is expected
        // timeout command can also return 124 if it times out
        if !status.success() && status.code() != Some(143) && status.code() != Some(124) {
            anyhow::bail!("Recording process failed with status: {}", status);
        }

        Ok(())
    }

    /// Check if the recording process is still running
    pub fn is_running(&mut self) -> bool {
        match self.process.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

/// Validate that a recording file exists and has content
pub fn validate_recording_file(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("Recording file does not exist: {}", path.display());
    }

    let metadata = std::fs::metadata(path)
        .context("Failed to read recording file metadata")?;

    if metadata.len() == 0 {
        anyhow::bail!("Recording file is empty");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_recording_config() {
        let config = RecordingConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.format, "s16");
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_validate_recording_file_missing() {
        let path = PathBuf::from("/tmp/nonexistent_file_12345.wav");
        let result = validate_recording_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_recording_file_empty() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        let result = validate_recording_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_validate_recording_file_valid() {
        let mut temp_file = NamedTempFile::new().unwrap();
        use std::io::Write;
        temp_file.write_all(b"some audio data").unwrap();
        temp_file.flush().unwrap();

        let path = temp_file.path().to_path_buf();
        let result = validate_recording_file(&path);
        assert!(result.is_ok());
    }

    // Note: We can't easily test actual recording without PipeWire,
    // so integration tests should be added separately
}
