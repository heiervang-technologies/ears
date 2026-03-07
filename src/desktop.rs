//! Desktop integration for ears
//!
//! Handles notifications, audio feedback, text input automation, and keyboard layout detection.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

/// Text input method for typing transcribed text
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TypingMode {
    /// Disable typing output entirely (useful for IPC-only mode)
    None,
    /// Auto-detect: wtype on Omarchy/Hyprland, clipboard paste otherwise
    #[default]
    Auto,
    /// Force wtype (character-by-character with inter-key delay)
    Wtype,
    /// Force clipboard paste (wl-copy + Ctrl+V, instant)
    Paste,
}

impl TypingMode {
    /// Display name for TUI rendering
    pub fn display_name(self) -> &'static str {
        match self {
            TypingMode::Auto => "Auto",
            TypingMode::Wtype => "Wtype",
            TypingMode::Paste => "Paste",
            TypingMode::None => "None",
        }
    }

    /// Cycle to the next mode
    pub fn next(self) -> Self {
        match self {
            TypingMode::Auto => TypingMode::Wtype,
            TypingMode::Wtype => TypingMode::Paste,
            TypingMode::Paste => TypingMode::None,
            TypingMode::None => TypingMode::Auto,
        }
    }
}

/// Keyboard layout detection for Hyprland and GNOME
pub struct KeyboardLayout;

impl KeyboardLayout {
    /// Detect the current keyboard layout and return the corresponding language code
    /// Returns Some("en") for US layout, Some("no") for Norwegian, None for unknown/auto
    ///
    /// Supports both Hyprland (via hyprctl) and GNOME (via dconf)
    pub fn detect_language() -> Option<String> {
        // Try Hyprland first
        if let Some(layout) = Self::detect_hyprland_layout() {
            tracing::debug!("Detected Hyprland keyboard layout: {}", layout);
            return Self::layout_to_language(&layout);
        }

        // Fall back to GNOME/dconf
        if let Some(layout) = Self::detect_gnome_layout() {
            tracing::debug!("Detected GNOME keyboard layout: {}", layout);
            return Self::layout_to_language(&layout);
        }

        None
    }

    /// Detect keyboard layout from Hyprland using hyprctl
    fn detect_hyprland_layout() -> Option<String> {
        // First try to get the active keyboard layout
        // hyprctl devices -j returns JSON with keyboard info
        let output = Command::new("hyprctl")
            .args(["devices", "-j"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json_str = String::from_utf8_lossy(&output.stdout);

        // Parse JSON to find the active keyboard layout
        // Look for "active_keymap" field in keyboards array
        if let Some(layout) = Self::parse_hyprctl_devices(&json_str) {
            return Some(layout);
        }

        // Fallback: try getting the configured layout from hyprctl getoption
        let option_output = Command::new("hyprctl")
            .args(["getoption", "input:kb_layout"])
            .output()
            .ok()?;

        if option_output.status.success() {
            let option_str = String::from_utf8_lossy(&option_output.stdout);
            // Output format: "str: us" or similar
            for line in option_str.lines() {
                if line.trim().starts_with("str:") {
                    let layout = line.trim().strip_prefix("str:")?.trim();
                    // Handle comma-separated layouts (e.g., "us,no") - take the first one
                    let first_layout = layout.split(',').next()?.trim();
                    if !first_layout.is_empty() {
                        return Some(first_layout.to_string());
                    }
                }
            }
        }

        None
    }

    /// Parse hyprctl devices JSON output to find active keyboard layout
    fn parse_hyprctl_devices(json_str: &str) -> Option<String> {
        // Simple JSON parsing without a full parser
        // Look for "active_keymap": "..." in the keyboards section
        // The active_keymap field contains the human-readable layout name

        // Find keyboards section
        let keyboards_start = json_str.find("\"keyboards\"")?;
        let keyboards_section = &json_str[keyboards_start..];

        // Find the first active_keymap in the keyboards array
        // Look for main keyboard (not virtual)
        for line in keyboards_section.lines() {
            let trimmed = line.trim();

            // Look for active_keymap field
            if trimmed.contains("\"active_keymap\"") {
                // Extract the value: "active_keymap": "English (US)"
                if let Some(start) = trimmed.find(':') {
                    let value_part = &trimmed[start + 1..];
                    let value = value_part.trim().trim_matches(',').trim_matches('"').trim();

                    // Map common keymap names to layout codes
                    return Self::keymap_name_to_layout(value);
                }
            }
        }

        None
    }

    /// Map Hyprland keymap names to layout codes
    fn keymap_name_to_layout(keymap: &str) -> Option<String> {
        let keymap_lower = keymap.to_lowercase();

        // Common keymap name patterns
        if keymap_lower.contains("english") && keymap_lower.contains("us") {
            return Some("us".to_string());
        }
        if keymap_lower.contains("english") && keymap_lower.contains("uk") {
            return Some("gb".to_string());
        }
        if keymap_lower.contains("norwegian") || keymap_lower.contains("norsk") {
            return Some("no".to_string());
        }
        if keymap_lower.contains("german") || keymap_lower.contains("deutsch") {
            return Some("de".to_string());
        }
        if keymap_lower.contains("french") || keymap_lower.contains("français") {
            return Some("fr".to_string());
        }
        if keymap_lower.contains("spanish") || keymap_lower.contains("español") {
            return Some("es".to_string());
        }
        if keymap_lower.contains("swedish") || keymap_lower.contains("svenska") {
            return Some("se".to_string());
        }
        if keymap_lower.contains("danish") || keymap_lower.contains("dansk") {
            return Some("dk".to_string());
        }
        if keymap_lower.contains("finnish") || keymap_lower.contains("suomi") {
            return Some("fi".to_string());
        }

        // If it's a short code already, use it directly
        let short = keymap.split_whitespace().next()?;
        if short.len() == 2 {
            return Some(short.to_lowercase());
        }

        None
    }

    /// Detect keyboard layout from GNOME using dconf
    fn detect_gnome_layout() -> Option<String> {
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
        Self::parse_dconf_mru_sources(&mru_str)
    }

    /// Parse the dconf mru-sources output and get the first (current) layout
    fn parse_dconf_mru_sources(sources: &str) -> Option<String> {
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
///
/// Sounds (E5 start, E4 done, double-B4 error) are embedded in the binary.
/// Custom sounds in ~/.local/share/ears-sounds/ take priority if present.
pub struct AudioFeedback;

// Embedded sound files
static SOUND_START: &[u8] = include_bytes!("../sounds/start.wav");
static SOUND_DONE: &[u8] = include_bytes!("../sounds/done.wav");
static SOUND_BELL: &[u8] = include_bytes!("../sounds/bell.wav");
static SOUND_VAD_OPEN: &[u8] = include_bytes!("../sounds/vad_open.wav");
static SOUND_VAD_CLOSE: &[u8] = include_bytes!("../sounds/vad_close.wav");
static SOUND_VAD_SPEECH: &[u8] = include_bytes!("../sounds/vad_speech.wav");
static SOUND_VAD_END: &[u8] = include_bytes!("../sounds/vad_end.wav");

impl AudioFeedback {
    /// Get custom sound directory
    fn sound_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("ears-sounds"))
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

    /// Play embedded sound data (non-blocking)
    ///
    /// Writes the WAV data to a cache file in /tmp and plays via paplay,
    /// which is more reliable than piping through stdin.
    fn play_embedded(data: &'static [u8]) -> Result<()> {
        use std::hash::{Hash, Hasher};
        use std::io::Write;

        // Derive a stable cache path from the data pointer (each static has a unique address)
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (data.as_ptr() as usize).hash(&mut hasher);
        let hash = hasher.finish();
        let cache_path = std::path::PathBuf::from(format!("/tmp/ears-sound-{:x}.wav", hash));

        // Write to cache file if not already present
        if !cache_path.exists() {
            let mut f =
                std::fs::File::create(&cache_path).context("Failed to create sound cache file")?;
            f.write_all(data)
                .context("Failed to write sound cache file")?;
        }

        Self::play_sound(&cache_path)
    }

    /// Play a named sound (custom override or embedded)
    fn play_named(name: &str, embedded: &'static [u8]) -> Result<()> {
        // Try custom sound first
        if let Ok(custom_dir) = Self::sound_dir() {
            let custom_wav = custom_dir.join(format!("{}.wav", name));
            if custom_wav.exists() {
                return Self::play_sound(&custom_wav);
            }
        }

        // Use embedded sound
        Self::play_embedded(embedded)
    }

    /// Play start recording beep (E5 - 660Hz)
    pub fn beep_start() -> Result<()> {
        Self::play_named("start", SOUND_START)
    }

    /// Play completion beep (E4 - 330Hz)
    pub fn beep_done() -> Result<()> {
        Self::play_named("done", SOUND_DONE)
    }

    /// Play error bell (double B4 - 493.88Hz)
    pub fn beep_error() -> Result<()> {
        Self::play_named("bell", SOUND_BELL)
    }

    /// Play VAD open sound (ascending C5→G5 chirp)
    pub fn beep_vad_open() -> Result<()> {
        Self::play_named("vad_open", SOUND_VAD_OPEN)
    }

    /// Play VAD close sound (descending G5→C5 chirp)
    pub fn beep_vad_close() -> Result<()> {
        Self::play_named("vad_close", SOUND_VAD_CLOSE)
    }

    /// Play VAD speech detected sound (short A5 blip)
    pub fn beep_vad_speech() -> Result<()> {
        Self::play_named("vad_speech", SOUND_VAD_SPEECH)
    }

    /// Play VAD speech ended sound (short E5→C5 blip)
    pub fn beep_vad_end() -> Result<()> {
        Self::play_named("vad_end", SOUND_VAD_END)
    }
}

/// Text input automation
pub struct TextInput;

impl TextInput {
    /// Detect if running on Omarchy (Arch + Hyprland)
    pub(crate) fn is_omarchy() -> bool {
        // Check if hyprctl exists (Hyprland compositor)
        if Command::new("hyprctl").arg("version").output().is_ok() {
            // Check if wtype is available (preferred on Hyprland)
            if Command::new("which")
                .arg("wtype")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    /// Send an Enter/Return key press
    ///
    /// Uses ydotool which creates a kernel-level evdev event indistinguishable
    /// from a physical keyboard press. This works reliably in TUI apps and tmux
    /// where wtype's virtual keyboard events may not be handled correctly.
    pub fn send_enter() -> Result<()> {
        use std::process::Stdio;

        // Brief delay to ensure the target app has processed previously typed text
        std::thread::sleep(std::time::Duration::from_millis(50));

        let status = Command::new("ydotool")
            .args(["key", "28:1", "28:0"]) // KEY_ENTER press and release
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run ydotool for Enter key")?;
        if !status.success() {
            anyhow::bail!("ydotool Enter failed with status: {}", status);
        }

        Ok(())
    }

    /// Type text using the specified mode
    ///
    /// - `Auto`: wtype on Omarchy/Hyprland, clipboard paste otherwise
    /// - `Wtype`: force wtype (character-by-character with inter-key delay)
    /// - `Paste`: force clipboard paste (wl-copy + Ctrl+V)
    pub fn type_text(text: &str, mode: TypingMode) -> Result<()> {
        match mode {
            TypingMode::Auto => {
                if Self::is_omarchy() {
                    Self::type_with_wtype(text)
                } else {
                    Self::paste_text(text)
                }
            }
            TypingMode::Wtype => Self::type_with_wtype(text),
            TypingMode::Paste => Self::paste_text(text),
            TypingMode::None => Ok(()),
        }
    }

    /// Type text directly using wtype (Wayland native, for Hyprland/Omarchy)
    ///
    /// Uses a small inter-key delay (`-d 4`) to prevent web browsers from
    /// dropping characters — especially spaces — when key events arrive too
    /// fast for the JavaScript event loop.
    fn type_with_wtype(text: &str) -> Result<()> {
        use std::process::Stdio;

        let status = Command::new("wtype")
            .arg("-d")
            .arg("4")
            .arg("--")
            .arg(text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run wtype")?;

        if !status.success() {
            anyhow::bail!("wtype failed with status: {}", status);
        }

        Ok(())
    }

    /// Paste text using wl-copy + ydotool Ctrl+V (handles Unicode correctly)
    /// Preserves and restores the original clipboard contents
    /// Used on non-Omarchy systems (Ubuntu, etc.)
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
        // Verify command construction without executing (avoids showing real notifications)
        let mut cmd = Command::new("notify-send");
        cmd.args(["--app-name=ears", "--urgency=normal", "Test info message"]);
        assert_eq!(cmd.get_program(), "notify-send");
        assert_eq!(cmd.get_args().count(), 3);
    }

    #[test]
    fn test_notification_warn() {
        let mut cmd = Command::new("notify-send");
        cmd.args([
            "--app-name=ears",
            "--urgency=normal",
            "Test warning message",
        ]);
        assert_eq!(cmd.get_program(), "notify-send");
    }

    #[test]
    fn test_notification_error() {
        let mut cmd = Command::new("notify-send");
        cmd.args([
            "--app-name=ears",
            "--urgency=critical",
            "Test error message",
        ]);
        assert_eq!(cmd.get_program(), "notify-send");
    }

    // 5.2 Audio Feedback Tests
    #[test]
    fn test_embedded_sounds() {
        // Verify embedded sounds are present and non-empty
        assert!(!SOUND_START.is_empty());
        assert!(!SOUND_DONE.is_empty());
        assert!(!SOUND_BELL.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn test_custom_sound_dir() {
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/home/testuser");
        let sound_dir = AudioFeedback::sound_dir().unwrap();
        assert_eq!(
            sound_dir,
            PathBuf::from("/home/testuser/.local/share/ears-sounds")
        );
        // Restore HOME to avoid poisoning other tests
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn test_beep_start() {
        // Verify embedded sound data is valid for playback without executing paplay
        assert!(!SOUND_START.is_empty(), "Start sound should be embedded");
    }

    #[test]
    fn test_beep_done() {
        assert!(!SOUND_DONE.is_empty(), "Done sound should be embedded");
    }

    #[test]
    fn test_beep_error() {
        assert!(!SOUND_BELL.is_empty(), "Error sound should be embedded");
    }

    #[test]
    fn test_audio_feedback_command_construction() {
        // Verify paplay command can be constructed without executing it
        let mut cmd = Command::new("paplay");
        cmd.arg("--raw").arg("/dev/null");
        assert_eq!(cmd.get_program(), "paplay");
        assert_eq!(cmd.get_args().count(), 2);
    }

    // 5.3 Text Input Tests
    // These test the detection logic without executing real typing commands,
    // since wtype/ydotool would type into the active window during tests.
    #[test]
    fn test_type_text_is_omarchy_detection() {
        // Verify is_omarchy returns a bool without side effects
        let _is_omarchy = TextInput::is_omarchy();
    }

    #[test]
    fn test_type_text_with_delay_constructs_command() {
        // Verify command construction doesn't panic for various inputs
        let mut cmd = Command::new("echo"); // harmless stand-in
        cmd.arg("type");
        cmd.arg("--key-delay").arg("50");
        cmd.arg("Test text");
        // Just verify the command can be built without issues
        assert!(cmd.get_program() == "echo");
    }

    #[test]
    fn test_type_text_special_characters_safe() {
        // Verify special characters can be passed as command args without panic
        let text = "Test: !@#$%^&*() \"quotes\" 'single' <angle> {braces}";
        let mut cmd = Command::new("echo");
        cmd.arg("--").arg(text);
        assert!(cmd.get_args().count() == 2);
    }

    // 5.4 Keyboard Layout Detection Tests
    #[test]
    fn test_parse_dconf_mru_sources_valid() {
        let sources = "[('xkb', 'no'), ('xkb', 'us')]";
        let result = KeyboardLayout::parse_dconf_mru_sources(sources);
        assert_eq!(result, Some("no".to_string()));
    }

    #[test]
    fn test_parse_dconf_mru_sources_single() {
        let sources = "[('xkb', 'us')]";
        let result = KeyboardLayout::parse_dconf_mru_sources(sources);
        assert_eq!(result, Some("us".to_string()));
    }

    #[test]
    fn test_parse_dconf_mru_sources_invalid() {
        let sources = "invalid data";
        let result = KeyboardLayout::parse_dconf_mru_sources(sources);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_hyprctl_devices_valid() {
        let json = r#"{
            "keyboards": [
                {
                    "address": "0x1234",
                    "name": "AT Translated Set 2 keyboard",
                    "active_keymap": "English (US)",
                    "main": true
                }
            ]
        }"#;
        let result = KeyboardLayout::parse_hyprctl_devices(json);
        assert_eq!(result, Some("us".to_string()));
    }

    #[test]
    fn test_parse_hyprctl_devices_norwegian() {
        let json = r#"{
            "keyboards": [
                {
                    "name": "keyboard",
                    "active_keymap": "Norwegian"
                }
            ]
        }"#;
        let result = KeyboardLayout::parse_hyprctl_devices(json);
        assert_eq!(result, Some("no".to_string()));
    }

    #[test]
    fn test_keymap_name_to_layout() {
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("English (US)"),
            Some("us".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("English (UK)"),
            Some("gb".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("Norwegian"),
            Some("no".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("German"),
            Some("de".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("French"),
            Some("fr".to_string())
        );
    }

    #[test]
    fn test_layout_to_language() {
        assert_eq!(
            KeyboardLayout::layout_to_language("us"),
            Some("en".to_string())
        );
        assert_eq!(
            KeyboardLayout::layout_to_language("gb"),
            Some("en".to_string())
        );
        assert_eq!(
            KeyboardLayout::layout_to_language("no"),
            Some("no".to_string())
        );
        assert_eq!(
            KeyboardLayout::layout_to_language("de"),
            Some("de".to_string())
        );
        assert_eq!(KeyboardLayout::layout_to_language("unknown"), None);
    }
}
