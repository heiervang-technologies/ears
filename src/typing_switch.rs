//! Persisted on/off switch for typing transcripts into the focused window.
//!
//! When typing is off, ears still transcribes and publishes every segment on
//! its IPC socket (`ears.sock`), so a consumer such as `talking-stick listen`
//! can route the words elsewhere without them also being typed. The switch
//! survives restarts: it lives in `$XDG_STATE_HOME/ears/typing` (override with
//! `EARS_TYPING_STATE`). A missing or unreadable file means typing is on.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::desktop::TypingMode;

/// A request carried by the `typing-*` command socket verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypingRequest {
    On,
    Off,
    Toggle,
    Status,
}

impl TypingRequest {
    /// Parse a command socket verb (`typing-on`, `typing-off`, `typing-toggle`, `typing-status`).
    pub fn from_command(verb: &str) -> Option<Self> {
        match verb {
            "typing-on" => Some(Self::On),
            "typing-off" => Some(Self::Off),
            "typing-toggle" => Some(Self::Toggle),
            "typing-status" => Some(Self::Status),
            _ => None,
        }
    }

    /// Parse the CLI argument (`on`, `off`, `toggle`, `status`).
    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::from_command(&format!("typing-{}", arg))
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::On => "typing-on",
            Self::Off => "typing-off",
            Self::Toggle => "typing-toggle",
            Self::Status => "typing-status",
        }
    }

    /// The resulting state, given the current one.
    pub fn apply(self, enabled: bool) -> bool {
        match self {
            Self::On => true,
            Self::Off => false,
            Self::Toggle => !enabled,
            Self::Status => enabled,
        }
    }

    pub fn changes_state(self) -> bool {
        self != Self::Status
    }
}

/// The wire/CLI form of a state: `typing:on` or `typing:off`.
pub fn describe(enabled: bool) -> String {
    format!("typing:{}", if enabled { "on" } else { "off" })
}

/// Where the switch is persisted.
pub fn state_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("EARS_TYPING_STATE") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    directories::ProjectDirs::from("com", "heiervang", "ears")
        .and_then(|p| p.state_dir().map(|d| d.join("typing")))
}

/// Whether typing is enabled. Anything but an explicit `off` means on.
pub fn load() -> bool {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim() != "off")
        .unwrap_or(true)
}

/// Persist the switch atomically.
pub fn save(enabled: bool) -> Result<()> {
    let path = state_path().context("No state directory for the typing switch")?;
    let dir = path.parent().context("Typing switch path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let tmp = dir.join(format!(".typing.{}.tmp", std::process::id()));
    std::fs::write(&tmp, if enabled { "on\n" } else { "off\n" })
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("Failed to replace {}: {}", path.display(), e)
    })
}

/// The mode the engine should use: the configured mode, or no typing at all.
pub fn effective_mode(configured: TypingMode, enabled: bool) -> TypingMode {
    if enabled {
        configured
    } else {
        TypingMode::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbs_and_args() {
        assert_eq!(
            TypingRequest::from_command("typing-off"),
            Some(TypingRequest::Off)
        );
        assert_eq!(
            TypingRequest::from_arg("toggle"),
            Some(TypingRequest::Toggle)
        );
        assert_eq!(
            TypingRequest::from_arg("status"),
            Some(TypingRequest::Status)
        );
        assert_eq!(TypingRequest::from_arg("sideways"), None);
        assert_eq!(TypingRequest::from_command("toggle-auto-enter"), None);
        for r in [
            TypingRequest::On,
            TypingRequest::Off,
            TypingRequest::Toggle,
            TypingRequest::Status,
        ] {
            assert_eq!(TypingRequest::from_command(r.command()), Some(r));
        }
    }

    #[test]
    fn apply_and_effective_mode() {
        assert!(TypingRequest::On.apply(false));
        assert!(!TypingRequest::Off.apply(true));
        assert!(TypingRequest::Toggle.apply(false));
        assert!(!TypingRequest::Toggle.apply(true));
        assert!(TypingRequest::Status.apply(true));
        assert!(!TypingRequest::Status.changes_state());
        assert_eq!(effective_mode(TypingMode::Wtype, true), TypingMode::Wtype);
        assert_eq!(effective_mode(TypingMode::Wtype, false), TypingMode::None);
        assert_eq!(describe(false), "typing:off");
    }

    #[test]
    fn persists_and_defaults_to_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("typing");
        // SAFETY: this is the only test touching EARS_TYPING_STATE.
        std::env::set_var("EARS_TYPING_STATE", &path);
        assert!(load(), "missing file means on");
        save(false).unwrap();
        assert!(!load());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "off\n");
        save(true).unwrap();
        assert!(load());
        std::fs::write(&path, "garbage").unwrap();
        assert!(load(), "anything but off means on");
        std::env::remove_var("EARS_TYPING_STATE");
    }
}
