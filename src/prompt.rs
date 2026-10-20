//! Shell-prompt indicator (`ears prompt`).
//!
//! Prints a short marker while ears is recording, transcribing or in VAD mode,
//! and nothing while idle, so prompt engines such as starship can hide the
//! segment. It only reads the state directory: no config load, no logging, no
//! locks, because it runs on every prompt.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// What the prompt should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptState {
    Recording,
    Transcribing,
    Vad,
    /// VAD listening for the wake phrase only.
    Standby,
}

/// File in the state directory that exists while VAD is in standby.
pub const STANDBY_MARKER: &str = "vad.standby";

impl PromptState {
    pub fn name(self) -> &'static str {
        match self {
            PromptState::Recording => "recording",
            PromptState::Transcribing => "transcribing",
            // One prompt segment covers VAD in either mode.
            PromptState::Vad | PromptState::Standby => "vad",
        }
    }
}

/// A transcription that has not finished in this long is treated as stale.
const TRANSCRIBING_STALE: Duration = Duration::from_secs(120);

fn pid_alive(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    match text.trim().parse::<i32>() {
        // Signal 0 only checks that the process exists.
        Ok(pid) if pid > 0 => unsafe { libc::kill(pid, 0) == 0 },
        _ => false,
    }
}

fn recently_modified(path: &Path, limit: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age <= limit)
}

/// Current state, cross-checked against live processes so a crashed ears
/// never leaves a stale marker in every prompt.
pub fn current(state_dir: &Path) -> Option<PromptState> {
    let state_file = state_dir.join("state");
    let state = fs::read_to_string(&state_file).ok()?;
    match state.trim() {
        "recording" if pid_alive(&state_dir.join("recording.pid")) => Some(PromptState::Recording),
        "transcribing" if recently_modified(&state_file, TRANSCRIBING_STALE) => {
            Some(PromptState::Transcribing)
        }
        // Headless VAD (`ears vad`) writes vad.pid; the TUI serves ears.sock.
        "vad_active"
            if pid_alive(&state_dir.join("vad.pid")) || crate::ipc::socket_path().exists() =>
        {
            if state_dir.join(STANDBY_MARKER).exists() {
                Some(PromptState::Standby)
            } else {
                Some(PromptState::Vad)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(state: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("state"), state).unwrap();
        dir
    }

    #[test]
    fn idle_and_missing_show_nothing() {
        assert_eq!(current(dir_with("idle").path()), None);
        assert_eq!(current(tempfile::tempdir().unwrap().path()), None);
        assert_eq!(current(dir_with("garbage").path()), None);
    }

    #[test]
    fn recording_needs_a_live_recorder() {
        let dir = dir_with("recording\n");
        assert_eq!(current(dir.path()), None, "no pid file");
        fs::write(
            dir.path().join("recording.pid"),
            std::process::id().to_string(),
        )
        .unwrap();
        assert_eq!(current(dir.path()), Some(PromptState::Recording));
        fs::write(dir.path().join("recording.pid"), "999999999").unwrap();
        assert_eq!(current(dir.path()), None, "dead recorder");
    }

    #[test]
    fn headless_vad_needs_a_live_vad_process() {
        let dir = dir_with("vad_active");
        fs::write(dir.path().join("vad.pid"), std::process::id().to_string()).unwrap();
        assert_eq!(current(dir.path()), Some(PromptState::Vad));
    }

    #[test]
    fn fresh_transcription_shows() {
        assert_eq!(
            current(dir_with("transcribing").path()),
            Some(PromptState::Transcribing)
        );
    }
}
