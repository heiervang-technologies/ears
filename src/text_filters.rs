//! Text filtering for transcription output
//!
//! This module provides configurable text transformations for transcribed text,
//! such as converting to lowercase or removing punctuation.

use serde::{Deserialize, Serialize};

fn default_alphabet_threshold() -> f32 {
    0.5
}

fn default_strict_alphabet() -> bool {
    true
}

/// Configuration for text filters
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextFilters {
    /// Convert text to lowercase
    #[serde(default)]
    pub lowercase: bool,
    /// Remove punctuation marks
    #[serde(default)]
    pub remove_punctuation: bool,
    /// Ignore transcription if it predominantly uses an alphabet different from the expected language
    #[serde(default = "default_strict_alphabet")]
    pub strict_alphabet: bool,
    /// Proportion of foreign characters allowed before ignoring the text (0.0 to 1.0)
    #[serde(default = "default_alphabet_threshold")]
    pub alphabet_threshold: f32,
}

impl Default for TextFilters {
    fn default() -> Self {
        Self {
            lowercase: false,
            remove_punctuation: false,
            strict_alphabet: true,
            alphabet_threshold: default_alphabet_threshold(),
        }
    }
}

impl TextFilters {
    /// Create a new TextFilters with default settings (all filters disabled)
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply all enabled filters to the input text
    pub fn apply(&self, text: &str, language: Option<&str>) -> String {
        let mut result = text.to_string();

        if self.strict_alphabet && self.exceeds_alphabet_threshold(&result, language) {
            return String::new();
        }

        if self.remove_punctuation {
            result = remove_punctuation(&result);
        }

        if self.lowercase {
            result = result.to_lowercase();
        }

        result
    }

    /// Check if any filter is enabled
    pub fn any_enabled(&self) -> bool {
        self.lowercase || self.remove_punctuation || self.strict_alphabet
    }

    /// Checks if the text has too many characters outside the expected script for the target language.
    fn exceeds_alphabet_threshold(&self, text: &str, language: Option<&str>) -> bool {
        if text.trim().is_empty() {
            return false;
        }

        // Only enforce for known Latin-based languages right now (can be expanded)
        let is_latin_lang = matches!(
            language.unwrap_or("en").to_lowercase().as_str(),
            "en" | "english"
                | "no"
                | "norwegian"
                | "nn"
                | "nb"
                | "es"
                | "spanish"
                | "fr"
                | "french"
                | "de"
                | "german"
        );

        if !is_latin_lang {
            return false;
        }

        let mut total_chars = 0;
        let mut foreign_chars = 0;

        for c in text.chars() {
            if c.is_alphabetic() {
                total_chars += 1;
                // Check if it's a Latin character (very roughly: ASCII or Latin-1 Supplement)
                // CJK, Thai, Arabic, Cyrillic, etc. will fail this.
                if !matches!(c, '\u{0000}'..='\u{024F}') {
                    foreign_chars += 1;
                }
            }
        }

        if total_chars == 0 {
            return false;
        }

        let proportion = foreign_chars as f32 / total_chars as f32;
        proportion > self.alphabet_threshold
    }
}

/// Remove common punctuation marks from text
fn remove_punctuation(text: &str) -> String {
    text.chars().filter(|c| !is_punctuation(*c)).collect()
}

/// Check if a character is a punctuation mark to remove
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | '!'
            | '?'
            | ';'
            | ':'
            | '"'
            | '\''
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '-'
            | '—'
            | '–'
            | '…'
            | '/'
            | '\\'
            | '&'
            | '*'
            | '#'
            | '@'
            | '%'
            | '^'
            | '+'
            | '='
            | '~'
            | '`'
            | '|'
            | '<'
            | '>'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_filters() {
        let filters = TextFilters::new();
        assert_eq!(filters.apply("Hello, World!", None), "Hello, World!");
    }

    #[test]
    fn test_lowercase() {
        let filters = TextFilters {
            lowercase: true,
            ..Default::default()
        };
        assert_eq!(filters.apply("Hello, World!", None), "hello, world!");
    }

    #[test]
    fn test_remove_punctuation() {
        let filters = TextFilters {
            remove_punctuation: true,
            ..Default::default()
        };
        assert_eq!(filters.apply("Hello, World!", None), "Hello World");
    }

    #[test]
    fn test_both_filters() {
        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: true,
            ..Default::default()
        };
        assert_eq!(filters.apply("Hello, World!", None), "hello world");
    }

    #[test]
    fn test_command_line_use_case() {
        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: true,
            ..Default::default()
        };
        // Typical ASR output that user wants to use in command line
        assert_eq!(
            filters.apply("Git commit -m \"Update readme.\"", None),
            "git commit m update readme"
        );
    }

    #[test]
    fn test_any_enabled() {
        let filters = TextFilters::new();
        // Since strict_alphabet defaults to true, any_enabled is now true by default
        assert!(filters.any_enabled());

        let mut filters = TextFilters {
            strict_alphabet: false,
            ..Default::default()
        };
        assert!(!filters.any_enabled());

        filters.lowercase = true;
        assert!(filters.any_enabled());

        let filters2 = TextFilters {
            remove_punctuation: true,
            ..Default::default()
        };
        assert!(filters2.any_enabled());
    }
}
