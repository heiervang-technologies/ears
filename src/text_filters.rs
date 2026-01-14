//! Text filtering for transcription output
//!
//! This module provides configurable text transformations for transcribed text,
//! such as converting to lowercase or removing punctuation.

use serde::{Deserialize, Serialize};

/// Configuration for text filters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextFilters {
    /// Convert text to lowercase
    pub lowercase: bool,
    /// Remove punctuation marks
    pub remove_punctuation: bool,
}

impl TextFilters {
    /// Create a new TextFilters with default settings (all filters disabled)
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply all enabled filters to the input text
    pub fn apply(&self, text: &str) -> String {
        let mut result = text.to_string();

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
        self.lowercase || self.remove_punctuation
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
        assert_eq!(filters.apply("Hello, World!"), "Hello, World!");
    }

    #[test]
    fn test_lowercase() {
        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: false,
        };
        assert_eq!(filters.apply("Hello, World!"), "hello, world!");
    }

    #[test]
    fn test_remove_punctuation() {
        let filters = TextFilters {
            lowercase: false,
            remove_punctuation: true,
        };
        assert_eq!(filters.apply("Hello, World!"), "Hello World");
    }

    #[test]
    fn test_both_filters() {
        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: true,
        };
        assert_eq!(filters.apply("Hello, World!"), "hello world");
    }

    #[test]
    fn test_command_line_use_case() {
        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: true,
        };
        // Typical ASR output that user wants to use in command line
        assert_eq!(
            filters.apply("Git commit -m \"Update readme.\""),
            "git commit m update readme"
        );
    }

    #[test]
    fn test_any_enabled() {
        let filters = TextFilters::new();
        assert!(!filters.any_enabled());

        let filters = TextFilters {
            lowercase: true,
            remove_punctuation: false,
        };
        assert!(filters.any_enabled());

        let filters = TextFilters {
            lowercase: false,
            remove_punctuation: true,
        };
        assert!(filters.any_enabled());
    }
}
