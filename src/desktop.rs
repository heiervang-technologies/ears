//! Desktop integration for ears
//!
//! Handles notifications, audio feedback, and text input automation.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Notification urgency levels
#[derive(Debug, Clone, Copy)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    fn as_str(&self) -> &str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        }
    }
}

/// Desktop notification manager
pub struct Notifications;

impl Notifications {
    /// Send a desktop notification
    pub fn send(message: &str, urgency: Urgency) -> Result<()> {
        Command::new("notify-send")
            .arg("-u")
            .arg(urgency.as_str())
            .arg("-a")
            .arg("ears")
            .arg(message)
            .output()
            .context("Failed to send notification")?;
        Ok(())
    }

    /// Send a low priority notification
    pub fn info(message: &str) -> Result<()> {
        Self::send(message, Urgency::Low)
    }

    /// Send a normal priority notification
    pub fn warn(message: &str) -> Result<()> {
        Self::send(message, Urgency::Normal)
    }

    /// Send a high priority notification
    pub fn error(message: &str) -> Result<()> {
        Self::send(message, Urgency::Critical)
    }
}

/// Audio feedback manager
pub struct AudioFeedback;

impl AudioFeedback {
    /// Get custom sound directory
    fn sound_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("ears-sounds"))
    }

    /// Get system sound directory
    fn system_sound_dir() -> PathBuf {
        PathBuf::from("/usr/share/sounds/freedesktop/stereo")
    }

    /// Play a sound file (non-blocking)
    fn play_sound(path: &PathBuf) -> Result<()> {
        Command::new("paplay")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to spawn paplay")?;
        Ok(())
    }

    /// Play a named sound
    pub fn play(sound: &str) -> Result<()> {
        // Try custom sound first
        if let Ok(custom_dir) = Self::sound_dir() {
            let custom_wav = custom_dir.join(format!("{}.wav", sound));
            if custom_wav.exists() {
                return Self::play_sound(&custom_wav);
            }
        }

        // Fall back to system sound
        let system_sound = Self::system_sound_dir().join(format!("{}.oga", sound));
        if system_sound.exists() {
            Self::play_sound(&system_sound)
        } else {
            // If sound doesn't exist, just silently succeed
            Ok(())
        }
    }

    /// Play start recording beep
    pub fn beep_start() -> Result<()> {
        Self::play("start")
    }

    /// Play completion beep
    pub fn beep_done() -> Result<()> {
        Self::play("done")
    }

    /// Play error bell
    pub fn beep_error() -> Result<()> {
        Self::play("bell")
    }
}

/// Text input automation
pub struct TextInput;

impl TextInput {
    /// Type text using ydotool
    pub fn type_text(text: &str) -> Result<()> {
        Command::new("ydotool")
            .arg("type")
            .arg(text)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to spawn ydotool")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urgency_conversion() {
        assert_eq!(Urgency::Low.as_str(), "low");
        assert_eq!(Urgency::Normal.as_str(), "normal");
        assert_eq!(Urgency::Critical.as_str(), "critical");
    }

    #[test]
    fn test_sound_paths() {
        let system_dir = AudioFeedback::system_sound_dir();
        assert_eq!(
            system_dir,
            PathBuf::from("/usr/share/sounds/freedesktop/stereo")
        );
    }
}
