use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

/// State of the ears daemon
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No active recording, ready to start
    Idle,
    /// Currently recording audio
    Recording,
    /// Processing/transcribing recorded audio
    Transcribing,
    /// VAD mode active - continuously listening and auto-transcribing
    VadActive,
}

/// Errors that can occur during state management
#[derive(Debug, Error)]
pub enum StateError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid state transition from {from:?} to {to:?}")]
    InvalidTransition { from: State, to: State },

    #[error("Recording timeout exceeded")]
    RecordingTimeout,

    #[error("State file corrupted")]
    CorruptedState,
}

/// Best-effort reset of state to idle and waybar notification.
///
/// Used by drop guards to ensure state is cleaned up on panic or early return.
pub fn force_reset_to_idle(state_dir: &Path) {
    let state_file = state_dir.join("state");
    let _ = fs::write(&state_file, "idle");
    let _ = std::process::Command::new("pkill")
        .args(["-RTMIN+9", "waybar"])
        .spawn();
}

/// Manages state transitions and persistence
pub struct StateManager {
    current_state: State,
    state_dir: PathBuf,
    recording_started: Option<Instant>,
    max_recording_duration: Duration,
}

impl StateManager {
    /// Create a new StateManager with the given state directory
    pub fn new<P: AsRef<Path>>(state_dir: P) -> Result<Self, StateError> {
        let state_dir = state_dir.as_ref().to_path_buf();

        // Ensure state directory exists
        fs::create_dir_all(&state_dir)?;

        Ok(Self {
            current_state: State::Idle,
            state_dir,
            recording_started: None,
            max_recording_duration: Duration::from_secs(120), // 2 minutes
        })
    }

    /// Get the current state
    pub fn current_state(&self) -> State {
        self.current_state
    }

    /// Transition to a new state
    pub fn transition(&mut self, new_state: State) -> Result<(), StateError> {
        // Validate state transition
        if !self.is_valid_transition(self.current_state, new_state) {
            return Err(StateError::InvalidTransition {
                from: self.current_state,
                to: new_state,
            });
        }

        // Check recording timeout before transitioning
        if self.current_state == State::Recording {
            if let Some(started) = self.recording_started {
                if started.elapsed() > self.max_recording_duration {
                    self.current_state = State::Idle;
                    self.recording_started = None;
                    return Err(StateError::RecordingTimeout);
                }
            }
        }

        // Update state
        self.current_state = new_state;

        // Track recording start time
        match new_state {
            State::Recording => {
                self.recording_started = Some(Instant::now());
            }
            State::Idle => {
                self.recording_started = None;
            }
            _ => {}
        }

        // Persist state to disk
        self.persist_state()?;

        Ok(())
    }

    /// Check if a state transition is valid
    fn is_valid_transition(&self, from: State, to: State) -> bool {
        match (from, to) {
            // Can only start recording from Idle
            (State::Idle, State::Recording) => true,
            // Can transition to transcribing from recording
            (State::Recording, State::Transcribing) => true,
            // Can enable VAD mode from Idle
            (State::Idle, State::VadActive) => true,
            // VAD mode can transition back to Idle
            (State::VadActive, State::Idle) => true,
            // Can always transition to Idle (emergency stop)
            (_, State::Idle) => true,
            // All other transitions are invalid
            _ => false,
        }
    }

    /// Get the path to the state file
    fn state_file_path(&self) -> PathBuf {
        self.state_dir.join("state")
    }

    /// Persist the current state to disk and notify waybar
    fn persist_state(&self) -> Result<(), StateError> {
        let state_str = match self.current_state {
            State::Idle => "idle",
            State::Recording => "recording",
            State::Transcribing => "transcribing",
            State::VadActive => "vad_active",
        };

        fs::write(self.state_file_path(), state_str)?;

        // Signal waybar to refresh the ears indicator (signal 9 = SIGRTMIN+9)
        let _ = std::process::Command::new("pkill")
            .args(["-RTMIN+9", "waybar"])
            .spawn();

        Ok(())
    }

    /// Load state from disk
    pub fn load_state(&mut self) -> Result<(), StateError> {
        let state_file = self.state_file_path();

        if !state_file.exists() {
            // No state file, default to Idle
            self.current_state = State::Idle;
            return Ok(());
        }

        let state_str = fs::read_to_string(&state_file)?;
        let state = match state_str.trim() {
            "idle" => State::Idle,
            "recording" => State::Recording,
            "transcribing" => State::Transcribing,
            "vad_active" => State::VadActive,
            _ => return Err(StateError::CorruptedState),
        };

        self.current_state = state;
        Ok(())
    }

    /// Check if recording has exceeded timeout
    pub fn check_recording_timeout(&self) -> bool {
        if self.current_state != State::Recording {
            return false;
        }

        if let Some(started) = self.recording_started {
            started.elapsed() > self.max_recording_duration
        } else {
            false
        }
    }

    /// Get the state directory path
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Reconcile state with actual process status
    ///
    /// This method checks if the current state is consistent with reality.
    /// If the state is Recording but no process is actually running, it resets to Idle.
    /// This prevents stale state after crashes or unexpected process termination.
    ///
    /// # Arguments
    /// * `is_process_alive` - Function that checks if the recording process is alive
    ///
    /// # Returns
    /// * `Ok(true)` if state was reconciled (changed from Recording to Idle)
    /// * `Ok(false)` if no reconciliation was needed
    /// * `Err` if there was an error during reconciliation
    pub fn reconcile_state<F>(&mut self, is_process_alive: F) -> Result<bool, StateError>
    where
        F: FnOnce() -> Result<bool, Box<dyn std::error::Error>>,
    {
        // Transcribing state is always stale on startup - it means a previous
        // process crashed or exited before resetting to Idle
        if self.current_state == State::Transcribing {
            tracing::warn!("Stale Transcribing state detected on startup, resetting to Idle");
            self.current_state = State::Idle;
            self.recording_started = None;
            self.persist_state()?;
            return Ok(true);
        }

        // Only need to reconcile Recording if process is dead
        if self.current_state != State::Recording {
            return Ok(false);
        }

        // Check if the recording process is actually alive
        let process_alive =
            is_process_alive().map_err(|e| StateError::Io(io::Error::other(e.to_string())))?;

        if !process_alive {
            // State says Recording but process is dead - reconcile
            tracing::warn!(
                "Stale Recording state detected on startup, resetting to Idle (process not running)"
            );

            // Reset to Idle without using transition() to avoid validation
            self.current_state = State::Idle;
            self.recording_started = None;
            self.persist_state()?;

            return Ok(true);
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_state_transitions() {
        let temp_dir = TempDir::new().unwrap();
        let mut manager = StateManager::new(temp_dir.path()).unwrap();

        // Start in Idle state
        assert_eq!(manager.current_state(), State::Idle);

        // Transition to Recording
        assert!(manager.transition(State::Recording).is_ok());
        assert_eq!(manager.current_state(), State::Recording);

        // Transition to Transcribing
        assert!(manager.transition(State::Transcribing).is_ok());
        assert_eq!(manager.current_state(), State::Transcribing);

        // Back to Idle
        assert!(manager.transition(State::Idle).is_ok());
        assert_eq!(manager.current_state(), State::Idle);
    }

    #[test]
    fn test_invalid_transitions() {
        let temp_dir = TempDir::new().unwrap();
        let mut manager = StateManager::new(temp_dir.path()).unwrap();

        // Can't go from Idle to Transcribing
        assert!(manager.transition(State::Transcribing).is_err());

        // Can't go from Recording to Recording
        manager.transition(State::Recording).unwrap();
        assert!(manager.transition(State::Recording).is_err());
    }

    #[test]
    fn test_state_persistence() {
        let temp_dir = TempDir::new().unwrap();

        // Create manager and transition to Recording
        {
            let mut manager = StateManager::new(temp_dir.path()).unwrap();
            manager.transition(State::Recording).unwrap();
        }

        // Create new manager and load state
        {
            let mut manager = StateManager::new(temp_dir.path()).unwrap();
            manager.load_state().unwrap();
            assert_eq!(manager.current_state(), State::Recording);
        }
    }

    #[test]
    fn test_recording_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let mut manager = StateManager::new(temp_dir.path()).unwrap();

        // Override timeout for testing
        manager.max_recording_duration = Duration::from_millis(100);

        // Start recording
        manager.transition(State::Recording).unwrap();

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(150));

        // Should timeout
        assert!(manager.check_recording_timeout());

        // Transition should fail with timeout error
        let result = manager.transition(State::Transcribing);
        assert!(matches!(result, Err(StateError::RecordingTimeout)));
        assert_eq!(manager.current_state(), State::Idle);
    }

    #[test]
    fn test_emergency_stop() {
        let temp_dir = TempDir::new().unwrap();
        let mut manager = StateManager::new(temp_dir.path()).unwrap();

        // Can always transition to Idle
        manager.transition(State::Recording).unwrap();
        assert!(manager.transition(State::Idle).is_ok());

        manager.transition(State::Recording).unwrap();
        manager.transition(State::Transcribing).unwrap();
        assert!(manager.transition(State::Idle).is_ok());
    }
}
