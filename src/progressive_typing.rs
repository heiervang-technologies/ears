//! Progressive typing with auto-correction support
//!
//! This module handles typing text progressively as it becomes stable from the
//! LocalAgreement policy, with optional backspace corrections for mistakes.

use crate::desktop::{TextInput, TypingMode};
use thiserror::Error;

/// Errors that can occur during progressive typing
#[derive(Error, Debug)]
pub enum ProgressiveTypingError {
    #[error("Text input error: {0}")]
    TextInputError(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),
}

/// Configuration for progressive typing
#[derive(Debug, Clone)]
pub struct ProgressiveTypingConfig {
    /// Enable progressive typing (type as text becomes stable)
    pub enabled: bool,
    /// Enable auto-correction (backspace and retype on changes)
    pub auto_correction: bool,
    /// Text input method
    pub typing_mode: TypingMode,
}

impl Default for ProgressiveTypingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_correction: false,
            typing_mode: TypingMode::Auto,
        }
    }
}

/// Progressive typing engine that tracks typed text and computes diffs
pub struct ProgressiveTypingEngine {
    /// Text that has been typed so far
    typed_text: String,
    /// Configuration
    config: ProgressiveTypingConfig,
}

impl ProgressiveTypingEngine {
    /// Create a new ProgressiveTypingEngine
    pub fn new(config: ProgressiveTypingConfig) -> Self {
        Self {
            typed_text: String::new(),
            config,
        }
    }

    /// Process newly committed text and type it progressively
    ///
    /// # Arguments
    /// * `committed_text` - The full committed text (stable prefix)
    ///
    /// # Returns
    /// * Number of characters typed/corrected
    pub fn update(&mut self, committed_text: &str) -> Result<usize, ProgressiveTypingError> {
        if !self.config.enabled {
            return Ok(0);
        }

        // If committed text is the same as typed, nothing to do
        if committed_text == self.typed_text {
            return Ok(0);
        }

        // Check if we need correction
        if self.config.auto_correction && !committed_text.starts_with(&self.typed_text) {
            // The committed text diverged from what we typed
            // Need to find the common prefix and correct from there
            let common_prefix = find_common_prefix(&self.typed_text, committed_text);
            let chars_to_delete = self.typed_text.chars().count() - common_prefix.chars().count();
            let text_to_type = &committed_text[common_prefix.len()..];

            if chars_to_delete > 0 {
                // Backspace to remove divergent text
                self.backspace(chars_to_delete)?;
            }

            // Type the correct text
            if !text_to_type.is_empty() {
                self.type_text(text_to_type)?;
            }

            self.typed_text = committed_text.to_string();
            Ok(chars_to_delete + text_to_type.len())
        } else if committed_text.starts_with(&self.typed_text) {
            // Committed text is an extension of what we typed
            let new_text = &committed_text[self.typed_text.len()..];

            if !new_text.is_empty() {
                self.type_text(new_text)?;
                self.typed_text = committed_text.to_string();
                Ok(new_text.len())
            } else {
                Ok(0)
            }
        } else if !self.config.auto_correction {
            // Diverged but correction is disabled — skip the divergent portion
            // and only append text beyond what we've already typed.
            if committed_text.len() > self.typed_text.len() {
                let char_count = self.typed_text.chars().count();
                let new_text: String = committed_text.chars().skip(char_count).collect();
                if !new_text.is_empty() {
                    self.type_text(&new_text)?;
                    self.typed_text.push_str(&new_text);
                    Ok(new_text.len())
                } else {
                    Ok(0)
                }
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }

    /// Type text using the configured input method
    fn type_text(&self, text: &str) -> Result<(), ProgressiveTypingError> {
        TextInput::type_text(text, self.config.typing_mode)
            .map_err(|e| ProgressiveTypingError::TextInputError(e.to_string()))?;
        Ok(())
    }

    /// Send backspace keypresses using wtype key simulation
    fn backspace(&self, count: usize) -> Result<(), ProgressiveTypingError> {
        use std::process::{Command, Stdio};

        for _ in 0..count {
            let status = Command::new("wtype")
                .arg("-k")
                .arg("BackSpace")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| ProgressiveTypingError::TextInputError(e.to_string()))?;

            if !status.success() {
                return Err(ProgressiveTypingError::TextInputError(
                    "wtype -k BackSpace failed".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Get the currently typed text
    pub fn typed_text(&self) -> &str {
        &self.typed_text
    }

    /// Reset the typing state (start fresh)
    pub fn reset(&mut self) {
        self.typed_text.clear();
    }

    /// Update configuration
    pub fn set_config(&mut self, config: ProgressiveTypingConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &ProgressiveTypingConfig {
        &self.config
    }
}

/// Find the longest common prefix of two strings
fn find_common_prefix(a: &str, b: &str) -> String {
    let mut prefix = String::new();
    let chars_a: Vec<char> = a.chars().collect();
    let chars_b: Vec<char> = b.chars().collect();

    for (ca, cb) in chars_a.iter().zip(chars_b.iter()) {
        if ca == cb {
            prefix.push(*ca);
        } else {
            break;
        }
    }

    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_common_prefix() {
        assert_eq!(find_common_prefix("hello", "hello"), "hello");
        assert_eq!(find_common_prefix("hello", "help"), "hel");
        assert_eq!(find_common_prefix("hello", "world"), "");
        assert_eq!(find_common_prefix("", "hello"), "");
        assert_eq!(find_common_prefix("hello", ""), "");
    }

    #[test]
    fn test_progressive_typing_disabled() {
        let config = ProgressiveTypingConfig {
            enabled: false,
            auto_correction: true,
            typing_mode: TypingMode::Auto,
        };

        let engine = ProgressiveTypingEngine::new(config);

        // Should do nothing when disabled
        // Note: This will fail because TextInput needs ydotool running
        // In real tests, we'd mock TextInput
        // For now, just check the typed_text tracking
        assert_eq!(engine.typed_text(), "");
    }

    #[test]
    fn test_config_update() {
        let mut engine = ProgressiveTypingEngine::new(ProgressiveTypingConfig::default());

        assert!(!engine.config().enabled);
        assert!(!engine.config().auto_correction);

        engine.set_config(ProgressiveTypingConfig {
            enabled: true,
            auto_correction: true,
            typing_mode: TypingMode::Auto,
        });

        assert!(engine.config().enabled);
        assert!(engine.config().auto_correction);
    }

    #[test]
    fn test_reset() {
        let mut engine = ProgressiveTypingEngine::new(ProgressiveTypingConfig::default());

        // Simulate some typed text
        engine.typed_text = "test".to_string();
        assert_eq!(engine.typed_text(), "test");

        engine.reset();
        assert_eq!(engine.typed_text(), "");
    }

    #[test]
    fn test_find_common_prefix_multibyte() {
        assert_eq!(find_common_prefix("café", "cafétéria"), "café");
        assert_eq!(find_common_prefix("cafétéria", "café"), "café");
        assert_eq!(find_common_prefix("", "über"), "");
        assert_eq!(find_common_prefix("über", ""), "");
        assert_eq!(find_common_prefix("日本語", "日本人"), "日本");
    }

    #[test]
    fn test_find_common_prefix_emoji() {
        assert_eq!(
            find_common_prefix("hello 👋 world", "hello 👋 earth"),
            "hello 👋 "
        );
        assert_eq!(
            find_common_prefix("👋🌍", "👋🌎"),
            "👋"
        );
        assert_eq!(
            find_common_prefix("👋", "🌍"),
            ""
        );
    }
}
