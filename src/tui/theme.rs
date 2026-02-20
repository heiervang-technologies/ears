//! Theme definitions for the TUI

use ratatui::style::Color;

/// Available themes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Dark,
    Light,
}

impl ThemeName {
    pub fn next(self) -> Self {
        match self {
            ThemeName::Dark => ThemeName::Light,
            ThemeName::Light => ThemeName::Dark,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "dark" => Some(ThemeName::Dark),
            "light" => Some(ThemeName::Light),
            _ => None,
        }
    }
}

/// Color palette for the TUI
#[derive(Debug, Clone)]
pub struct Theme {
    /// Header title color
    pub title: Color,
    /// Normal text
    pub text: Color,
    /// Dimmed/secondary text
    pub dim: Color,
    /// Accent color (selected tabs, highlights)
    pub accent: Color,
    /// Status panel border
    pub border_status: Color,
    /// Config panel border
    pub border_config: Color,
    /// Logs panel border
    pub border_logs: Color,
    /// Live panel border
    pub border_live: Color,
    /// Active/editing border
    pub border_active: Color,
    /// Header border
    pub border_header: Color,
    /// Recording indicator
    pub recording: Color,
    /// Success/active indicator
    pub success: Color,
    /// Error indicator
    pub error: Color,
    /// Search match highlight background
    pub search_match_bg: Color,
    /// Search match highlight foreground
    pub search_match_fg: Color,
    /// Checkbox/toggle color
    pub toggle: Color,
    /// Env var indicator color
    pub env_indicator: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            title: Color::Cyan,
            text: Color::White,
            dim: Color::DarkGray,
            accent: Color::Yellow,
            border_status: Color::Green,
            border_config: Color::Blue,
            border_logs: Color::Magenta,
            border_live: Color::Green,
            border_active: Color::Yellow,
            border_header: Color::White,
            recording: Color::Red,
            success: Color::Green,
            error: Color::Red,
            search_match_bg: Color::Red,
            search_match_fg: Color::White,
            toggle: Color::Cyan,
            env_indicator: Color::Yellow,
        }
    }

    pub fn light() -> Self {
        Self {
            title: Color::Blue,
            text: Color::Black,
            dim: Color::DarkGray,
            accent: Color::Blue,
            border_status: Color::Green,
            border_config: Color::Blue,
            border_logs: Color::Magenta,
            border_live: Color::Green,
            border_active: Color::Red,
            border_header: Color::DarkGray,
            recording: Color::Red,
            success: Color::Green,
            error: Color::Red,
            search_match_bg: Color::Yellow,
            search_match_fg: Color::Black,
            toggle: Color::Blue,
            env_indicator: Color::Magenta,
        }
    }

    pub fn from_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self::dark(),
            ThemeName::Light => Self::light(),
        }
    }
}
