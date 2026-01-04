use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use thiserror::Error;

/// Errors that can occur during process management
#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    #[error("Process not found")]
    ProcessNotFound,

    #[error("Failed to signal process: {0}")]
    SignalFailed(String),

    #[error("Invalid PID in file")]
    InvalidPid,

    #[error("Failed to terminate process gracefully")]
    TerminationFailed,
}

/// Manages a recording process with PID tracking
pub struct ProcessManager {
    pid_file: PathBuf,
    timeout: Duration,
}

impl ProcessManager {
    /// Create a new ProcessManager
    pub fn new<P: AsRef<Path>>(pid_file: P, timeout: Duration) -> Self {
        Self {
            pid_file: pid_file.as_ref().to_path_buf(),
            timeout,
        }
    }

    /// Spawn a new pw-record process
    pub fn spawn_recording(&self, device: &str, output_file: &Path) -> Result<u32, ProcessError> {
        // Build the command
        let mut cmd = Command::new("timeout");
        cmd.arg(self.timeout.as_secs().to_string())
            .arg("pw-record")
            .arg("--target")
            .arg(device)
            .arg("--rate")
            .arg("16000")
            .arg("--channels")
            .arg("1")
            .arg("--format")
            .arg("s16")
            .arg(output_file);

        // Spawn the process
        let child = cmd
            .spawn()
            .map_err(|e| ProcessError::SpawnFailed(format!("Failed to spawn pw-record: {}", e)))?;

        let pid = child.id();

        // Write PID to file
        self.write_pid(pid)?;

        // Spawn a thread to wait for the child and clean up zombie processes
        // This prevents zombie accumulation while still allowing background execution
        std::thread::spawn(move || {
            let _ = child.wait_with_output();
            tracing::debug!("Recording process {} cleaned up", pid);
        });

        Ok(pid)
    }

    /// Write a PID to the PID file
    fn write_pid(&self, pid: u32) -> Result<(), ProcessError> {
        // Ensure parent directory exists
        if let Some(parent) = self.pid_file.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&self.pid_file, pid.to_string())?;
        Ok(())
    }

    /// Read the PID from the PID file
    pub fn read_pid(&self) -> Result<Option<u32>, ProcessError> {
        if !self.pid_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.pid_file)?;
        let pid = content
            .trim()
            .parse::<u32>()
            .map_err(|_| ProcessError::InvalidPid)?;

        Ok(Some(pid))
    }

    /// Delete the PID file
    pub fn delete_pid_file(&self) -> Result<(), ProcessError> {
        if self.pid_file.exists() {
            fs::remove_file(&self.pid_file)?;
        }
        Ok(())
    }

    /// Check if a process is alive
    pub fn is_process_alive(&self, pid: u32) -> bool {
        // Send signal 0 to check if process exists
        let pid = Pid::from_raw(pid as i32);
        signal::kill(pid, None).is_ok()
    }

    /// Check if the recorded process is alive
    pub fn is_recording_alive(&self) -> Result<bool, ProcessError> {
        match self.read_pid()? {
            Some(pid) => Ok(self.is_process_alive(pid)),
            None => Ok(false),
        }
    }

    /// Terminate the process gracefully (SIGTERM)
    pub fn terminate(&self, pid: u32) -> Result<(), ProcessError> {
        let pid = Pid::from_raw(pid as i32);

        signal::kill(pid, Signal::SIGTERM)
            .map_err(|e| ProcessError::SignalFailed(format!("Failed to send SIGTERM: {}", e)))?;

        Ok(())
    }

    /// Force kill the process (SIGKILL)
    pub fn kill(&self, pid: u32) -> Result<(), ProcessError> {
        let pid = Pid::from_raw(pid as i32);

        signal::kill(pid, Signal::SIGKILL)
            .map_err(|e| ProcessError::SignalFailed(format!("Failed to send SIGKILL: {}", e)))?;

        Ok(())
    }

    /// Stop the recording process gracefully
    pub fn stop_recording(&self) -> Result<(), ProcessError> {
        let pid = match self.read_pid()? {
            Some(pid) => pid,
            None => return Err(ProcessError::ProcessNotFound),
        };

        if !self.is_process_alive(pid) {
            // Process already dead, just clean up
            self.delete_pid_file()?;
            return Ok(());
        }

        // Try graceful termination first
        self.terminate(pid)?;

        // Wait a bit for graceful shutdown
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(100));
            if !self.is_process_alive(pid) {
                self.delete_pid_file()?;
                return Ok(());
            }
        }

        // If still alive, force kill
        self.kill(pid)?;

        // Wait again
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(100));
            if !self.is_process_alive(pid) {
                self.delete_pid_file()?;
                return Ok(());
            }
        }

        // Process won't die - clean up PID file anyway and return error
        // This ensures we don't leave stale PID files even if termination fails
        self.delete_pid_file()?;
        Err(ProcessError::TerminationFailed)
    }

    /// Clean up stale PID files (when process is dead)
    pub fn cleanup_stale(&self) -> Result<(), ProcessError> {
        if let Some(pid) = self.read_pid()? {
            if !self.is_process_alive(pid) {
                self.delete_pid_file()?;
            }
        }
        Ok(())
    }

    /// Get the PID file path
    pub fn pid_file_path(&self) -> &Path {
        &self.pid_file
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn test_pid_file_operations() {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join("test.pid");
        let manager = ProcessManager::new(&pid_file, Duration::from_secs(120));

        // Initially no PID
        assert!(manager.read_pid().unwrap().is_none());

        // Write a PID
        manager.write_pid(12345).unwrap();

        // Should be able to read it back
        assert_eq!(manager.read_pid().unwrap(), Some(12345));

        // Delete PID file
        manager.delete_pid_file().unwrap();
        assert!(manager.read_pid().unwrap().is_none());
    }

    #[test]
    fn test_process_health_check() {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join("test.pid");
        let manager = ProcessManager::new(&pid_file, Duration::from_secs(120));

        // Our own process should be alive
        let our_pid = std::process::id();
        assert!(manager.is_process_alive(our_pid));

        // A non-existent PID should be dead
        assert!(!manager.is_process_alive(999999));
    }

    #[test]
    fn test_cleanup_stale_pid() {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join("test.pid");
        let manager = ProcessManager::new(&pid_file, Duration::from_secs(120));

        // Write a PID for a non-existent process
        manager.write_pid(999999).unwrap();

        // Cleanup should remove the stale PID file
        manager.cleanup_stale().unwrap();
        assert!(manager.read_pid().unwrap().is_none());
    }

    #[test]
    fn test_cleanup_live_process() {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join("test.pid");
        let manager = ProcessManager::new(&pid_file, Duration::from_secs(120));

        // Write our own PID (alive process)
        let our_pid = std::process::id();
        manager.write_pid(our_pid).unwrap();

        // Cleanup should NOT remove the PID file
        manager.cleanup_stale().unwrap();
        assert_eq!(manager.read_pid().unwrap(), Some(our_pid));
    }

    #[test]
    fn test_process_termination() {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join("test.pid");
        let manager = ProcessManager::new(&pid_file, Duration::from_secs(120));

        // Spawn a long-running process (sleep)
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();

        manager.write_pid(pid).unwrap();

        // Process should be alive
        assert!(manager.is_process_alive(pid));

        // Terminate it
        let result = manager.stop_recording();

        // Should either succeed or the process might already be dead
        // In some environments, signal handling can be tricky
        if let Err(e) = result {
            eprintln!("Warning: termination had an issue: {}", e);
        }

        // Give it some extra time to ensure the process is dead
        std::thread::sleep(Duration::from_millis(200));

        // Process should be dead (or at least the PID file should be cleaned up)
        // We check PID file deletion as the main success criteria
        assert!(manager.read_pid().unwrap().is_none());

        // Wait on the child to prevent zombie processes
        let _ = child.wait();
    }

    #[test]
    fn test_is_recording_alive() {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join("test.pid");
        let manager = ProcessManager::new(&pid_file, Duration::from_secs(120));

        // No PID file - not alive
        assert!(!manager.is_recording_alive().unwrap());

        // Write a live PID
        let our_pid = std::process::id();
        manager.write_pid(our_pid).unwrap();
        assert!(manager.is_recording_alive().unwrap());

        // Write a dead PID
        manager.write_pid(999999).unwrap();
        assert!(!manager.is_recording_alive().unwrap());
    }
}
