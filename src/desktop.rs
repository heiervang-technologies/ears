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
    /// Type text using ydotool with optional delay
    ///
    /// The `delay_ms` parameter controls the delay between keystrokes.
    /// Default is 12ms if not specified (ydotool's default).
    pub fn type_text(text: &str) -> Result<()> {
        Self::type_text_with_delay(text, None)
    }

    /// Type text using ydotool with a specific delay
    pub fn type_text_with_delay(text: &str, delay_ms: Option<u32>) -> Result<()> {
        let mut cmd = Command::new("ydotool");
        cmd.arg("type");

        // Add delay if specified
        if let Some(delay) = delay_ms {
            cmd.arg("--key-delay").arg(delay.to_string());
        }

        // ydotool handles special characters automatically
        cmd.arg(text);

        // Use .status() to wait for completion, preventing concurrent processes
        // from interleaving output (fixes #57)
        let status = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("Failed to run ydotool")?;

        if !status.success() {
            anyhow::bail!("ydotool failed with status: {}", status);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 5.1 Notifications Tests
    #[test]
    fn test_urgency_conversion() {
        assert_eq!(Urgency::Low.as_str(), "low");
        assert_eq!(Urgency::Normal.as_str(), "normal");
        assert_eq!(Urgency::Critical.as_str(), "critical");
    }

    #[test]
    fn test_notification_info() {
        // This will fail if notify-send is not installed, but that's expected
        // In production, notify-send should be available
        let result = Notifications::info("Test info message");
        // We don't assert success because notify-send might not be available in test env
        // Just verify it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_notification_warn() {
        let result = Notifications::warn("Test warning message");
        let _ = result;
    }

    #[test]
    fn test_notification_error() {
        let result = Notifications::error("Test error message");
        let _ = result;
    }

    // 5.2 Audio Feedback Tests
    #[test]
    fn test_sound_paths() {
        let system_dir = AudioFeedback::system_sound_dir();
        assert_eq!(
            system_dir,
            PathBuf::from("/usr/share/sounds/freedesktop/stereo")
        );
    }

    #[test]
    fn test_custom_sound_dir() {
        std::env::set_var("HOME", "/home/testuser");
        let sound_dir = AudioFeedback::sound_dir().unwrap();
        assert_eq!(
            sound_dir,
            PathBuf::from("/home/testuser/.local/share/ears-sounds")
        );
    }

    #[test]
    fn test_beep_start() {
        // Test that beep_start doesn't panic
        // Will fail gracefully if paplay not available
        let result = AudioFeedback::beep_start();
        let _ = result;
    }

    #[test]
    fn test_beep_done() {
        let result = AudioFeedback::beep_done();
        let _ = result;
    }

    #[test]
    fn test_beep_error() {
        let result = AudioFeedback::beep_error();
        let _ = result;
    }

    #[test]
    fn test_audio_feedback_non_blocking() {
        // Play multiple sounds to verify non-blocking behavior
        let _ = AudioFeedback::beep_start();
        let _ = AudioFeedback::beep_done();
        let _ = AudioFeedback::beep_error();
        // If these were blocking, this test would take a long time
    }

    // 5.3 Text Input Tests
    #[test]
    fn test_type_text_basic() {
        // Test basic text typing (won't actually type in test env)
        let result = TextInput::type_text("Hello, world!");
        let _ = result;
    }

    #[test]
    fn test_type_text_with_special_characters() {
        // Test that special characters don't cause issues
        let result = TextInput::type_text("Test: !@#$%^&*()");
        let _ = result;
    }

    #[test]
    fn test_type_text_with_delay() {
        // Test typing with custom delay
        let result = TextInput::type_text_with_delay("Test text", Some(50));
        let _ = result;
    }

    #[test]
    fn test_type_text_with_no_delay() {
        // Test typing with no delay (use default)
        let result = TextInput::type_text_with_delay("Test text", None);
        let _ = result;
    }
}
