//! State management for ears
//!
//! Handles runtime state including PID files, lock files, and recording state.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Runtime state management
#[derive(Debug)]
pub struct State {
    /// State directory path
    pub state_dir: PathBuf,
}

impl State {
    /// Create a new State manager
    pub fn new() -> Result<Self> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));

        let state_dir = PathBuf::from(runtime_dir).join("ears");
        fs::create_dir_all(&state_dir).context("Failed to create state directory")?;

        Ok(Self { state_dir })
    }

    /// Get the lock file path
    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join("lock")
    }

    /// Get the PID file path
    pub fn pid_file(&self) -> PathBuf {
        self.state_dir.join("recording.pid")
    }

    /// Get the audio file path
    pub fn audio_file(&self) -> PathBuf {
        self.state_dir.join("recording.wav")
    }

    /// Get the debug log file path
    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("debug.log")
    }

    /// Check if a recording is currently active
    pub fn is_recording(&self) -> bool {
        let pid_file = self.pid_file();
        if !pid_file.exists() {
            return false;
        }

        // Read PID and check if process is alive
        if let Ok(pid_str) = fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                // Check if process exists (signal 0 doesn't kill, just checks)
                unsafe { libc::kill(pid, 0) == 0 }
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Get the PID of the recording process
    pub fn get_recording_pid(&self) -> Option<i32> {
        let pid_file = self.pid_file();
        if !pid_file.exists() {
            return None;
        }

        fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
    }

    /// Clean up stale PID files
    pub fn cleanup_stale(&self) -> Result<()> {
        if !self.is_recording() {
            let pid_file = self.pid_file();
            let audio_file = self.audio_file();

            if pid_file.exists() {
                fs::remove_file(pid_file).ok();
            }
            if audio_file.exists() {
                fs::remove_file(audio_file).ok();
            }
        }
        Ok(())
    }

    /// Save PID to file
    pub fn save_pid(&self, pid: i32) -> Result<()> {
        let pid_file = self.pid_file();
        fs::write(&pid_file, pid.to_string()).context("Failed to write PID file")?;
        Ok(())
    }

    /// Remove PID file
    pub fn remove_pid(&self) -> Result<()> {
        let pid_file = self.pid_file();
        if pid_file.exists() {
            fs::remove_file(&pid_file).context("Failed to remove PID file")?;
        }
        Ok(())
    }

    /// Remove audio file
    pub fn remove_audio(&self) -> Result<()> {
        let audio_file = self.audio_file();
        if audio_file.exists() {
            fs::remove_file(&audio_file).context("Failed to remove audio file")?;
        }
        Ok(())
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new().expect("Failed to create state directory")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_state_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        env::set_var("XDG_RUNTIME_DIR", temp_dir.path());

        let state = State::new().unwrap();
        assert!(state.state_dir.exists());
    }

    #[test]
    fn test_pid_operations() {
        let temp_dir = tempfile::tempdir().unwrap();
        env::set_var("XDG_RUNTIME_DIR", temp_dir.path());

        let state = State::new().unwrap();

        // Initially no recording
        assert!(!state.is_recording());
        assert_eq!(state.get_recording_pid(), None);

        // Save our own PID (we know we're running)
        let pid = std::process::id() as i32;
        state.save_pid(pid).unwrap();

        // Should now detect recording
        assert!(state.is_recording());
        assert_eq!(state.get_recording_pid(), Some(pid));

        // Remove PID
        state.remove_pid().unwrap();
        assert!(!state.is_recording());
    }

    #[test]
    fn test_cleanup_stale() {
        let temp_dir = tempfile::tempdir().unwrap();
        env::set_var("XDG_RUNTIME_DIR", temp_dir.path());

        let state = State::new().unwrap();

        // Create stale PID file (non-existent process)
        state.save_pid(999999).unwrap();
        fs::write(state.audio_file(), b"fake audio").unwrap();

        // Cleanup should remove both
        state.cleanup_stale().unwrap();
        assert!(!state.pid_file().exists());
        assert!(!state.audio_file().exists());
    }
}
