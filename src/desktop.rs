//! Desktop integration for ears
//!
//! Handles notifications, audio feedback, text input automation, and keyboard layout detection.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Keyboard layout detection for GNOME
pub struct KeyboardLayout;

impl KeyboardLayout {
    /// Detect the current keyboard layout and return the corresponding language code
    /// Returns Some("en") for US layout, Some("no") for Norwegian, None for unknown/auto
    pub fn detect_language() -> Option<String> {
        // Use mru-sources (most recently used) - first item is current layout
        // This works with GNOME's per-window keyboard layout switching
        let mru_output = Command::new("dconf")
            .args(["read", "/org/gnome/desktop/input-sources/mru-sources"])
            .output()
            .ok()?;

        if !mru_output.status.success() {
            return None;
        }

        let mru_str = String::from_utf8_lossy(&mru_output.stdout);
        // Parse "[('xkb', 'no'), ('xkb', 'us')]" - first entry is current
        let layout = Self::parse_first_layout(&mru_str)?;

        tracing::debug!("Detected keyboard layout: {}", layout);

        // Map layout code to language code
        Self::layout_to_language(&layout)
    }

    /// Parse the dconf mru-sources output and get the first (current) layout
    fn parse_first_layout(sources: &str) -> Option<String> {
        // Format: [('xkb', 'no'), ('xkb', 'us')]
        // First entry is the current layout

        let trimmed = sources.trim();
        if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
            return None;
        }

        // Find the first 'layout' pattern
        if let Some(start) = trimmed.find("('xkb', '") {
            let after_prefix = &trimmed[start + 9..]; // Skip "('xkb', '"
            if let Some(end) = after_prefix.find("')") {
                return Some(after_prefix[..end].to_string());
            }
        }

        None
    }

    /// Map keyboard layout code to transcription language code
    fn layout_to_language(layout: &str) -> Option<String> {
        match layout {
            "us" | "gb" | "uk" => Some("en".to_string()),
            "no" | "no+nodeadkeys" => Some("no".to_string()),
            "de" => Some("de".to_string()),
            "fr" => Some("fr".to_string()),
            "es" => Some("es".to_string()),
            "se" => Some("sv".to_string()),
            "dk" => Some("da".to_string()),
            "fi" => Some("fi".to_string()),
            // Add more mappings as needed
            _ => None, // Unknown layout = auto-detect
        }
    }
}

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
        // Use clipboard + paste for reliable Unicode support
        Self::paste_text(text)
    }

    /// Paste text using wl-copy + ydotool Ctrl+V (handles Unicode correctly)
    /// Preserves and restores the original clipboard contents
    fn paste_text(text: &str) -> Result<()> {
        use std::process::Stdio;

        // Save current clipboard contents
        let original_clipboard = Command::new("wl-paste")
            .arg("--no-newline")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(o.stdout)
                } else {
                    None
                }
            });

        // Copy text to clipboard using wl-copy
        let mut child = Command::new("wl-copy")
            .arg("--")
            .arg(text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to run wl-copy")?;

        child.wait().context("wl-copy failed")?;

        // Small delay to ensure clipboard is ready
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Simulate Ctrl+V to paste
        let status = Command::new("ydotool")
            .args(["key", "ctrl+v"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run ydotool key")?;

        if !status.success() {
            anyhow::bail!("ydotool key failed with status: {}", status);
        }

        // Small delay before restoring clipboard
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Restore original clipboard contents
        if let Some(original) = original_clipboard {
            let mut restore = Command::new("wl-copy")
                .arg("--")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("Failed to restore clipboard")?;

            if let Some(mut stdin) = restore.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(&original);
            }
            let _ = restore.wait();
        }

        Ok(())
    }

    /// Type text using ydotool with a specific delay (fallback)
    #[allow(dead_code)]
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
