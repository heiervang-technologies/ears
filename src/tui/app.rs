//! TUI application state and logic

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::config::Config;

/// The current panel being displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Status,
    Configuration,
    Logs,
}

impl Panel {
    /// Get the next panel (tab right)
    pub fn next(self) -> Self {
        match self {
            Panel::Status => Panel::Configuration,
            Panel::Configuration => Panel::Logs,
            Panel::Logs => Panel::Status,
        }
    }

    /// Get the previous panel (tab left)
    pub fn prev(self) -> Self {
        match self {
            Panel::Status => Panel::Logs,
            Panel::Configuration => Panel::Status,
            Panel::Logs => Panel::Configuration,
        }
    }

    /// Get the panel title
    pub fn title(self) -> &'static str {
        match self {
            Panel::Status => "Status",
            Panel::Configuration => "Configuration",
            Panel::Logs => "Logs",
        }
    }
}

/// Main application state for the TUI
#[derive(Clone)]
pub struct App {
    /// Current active panel
    pub current_panel: Panel,
    /// Whether the app is in command mode (vim :command)
    pub command_mode: bool,
    /// Current command being typed
    pub command_buffer: String,
    /// Recording state (for display)
    pub is_recording: bool,
    /// Recording duration in seconds
    pub recording_duration: u64,
    /// Tick counter for tracking recording duration (4 ticks = 1 second at 250ms tick rate)
    tick_count: u64,
    /// Current model name
    pub model: String,
    /// Server URL
    pub server: String,
    /// Audio device name
    pub device: String,
    /// Log messages
    pub logs: Vec<String>,
    /// Selected log index (for scrolling)
    pub selected_log: usize,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Self {
        // Load config from files
        let config = Config::load().unwrap_or_default();
        let server_url = config.whisper_server.to_string();
        let device = config.device.clone();

        // Try to fetch model from the server's /v1/models endpoint
        let model = Self::fetch_model_name(&server_url).unwrap_or_else(|| "unknown".to_string());

        Self {
            current_panel: Panel::Status,
            command_mode: false,
            command_buffer: String::new(),
            is_recording: false,
            recording_duration: 0,
            tick_count: 0,
            model,
            server: server_url,
            device,
            logs: vec![
                "Application started".to_string(),
                "TUI initialized".to_string(),
            ],
            selected_log: 0,
        }
    }

    /// Fetch the model name from the whisper server
    fn fetch_model_name(server_url: &str) -> Option<String> {
        let base = server_url.trim_end_matches('/');
        let url = format!("{}/v1/models", base);

        // Synchronous blocking request (TUI init is sync)
        let response = reqwest::blocking::get(&url).ok()?;
        let json: serde_json::Value = response.json().ok()?;

        // OpenAI-compatible response: {"data": [{"id": "model-name", ...}]}
        json.get("data")?
            .as_array()?
            .first()?
            .get("id")?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Handle a key press event
    /// Returns false if the app should quit
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Handle command mode separately
        if self.command_mode {
            return self.handle_command_key(key);
        }

        // Global keybindings
        match (key.code, key.modifiers) {
            // Quit with 'q' or Ctrl+C
            (KeyCode::Char('q'), KeyModifiers::NONE) => return Ok(false),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(false),

            // Enter command mode with ':'
            (KeyCode::Char(':'), KeyModifiers::NONE) => {
                self.command_mode = true;
                self.command_buffer.clear();
            }

            // Tab navigation with h/l (vim-style)
            (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.current_panel = self.current_panel.prev();
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) => {
                self.current_panel = self.current_panel.next();
            }

            // Tab navigation with Tab/Shift+Tab
            (KeyCode::Tab, KeyModifiers::NONE) => {
                self.current_panel = self.current_panel.next();
            }
            (KeyCode::BackTab, KeyModifiers::SHIFT) => {
                self.current_panel = self.current_panel.prev();
            }

            // Panel-specific navigation with j/k (vim-style)
            (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.scroll_down();
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.scroll_up();
            }

            // Space to toggle recording
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                self.toggle_recording();
            }

            // 'c' to go to configuration panel
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.current_panel = Panel::Configuration;
            }

            _ => {}
        }

        Ok(true)
    }

    /// Handle key press in command mode
    fn handle_command_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.command_mode = false;
                self.command_buffer.clear();
            }
            KeyCode::Enter => {
                let should_continue = self.execute_command()?;
                self.command_mode = false;
                self.command_buffer.clear();
                return Ok(should_continue);
            }
            KeyCode::Char(c) => {
                self.command_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.command_buffer.pop();
            }
            _ => {}
        }
        Ok(true)
    }

    /// Execute a vim-style command
    fn execute_command(&mut self) -> Result<bool> {
        let cmd = self.command_buffer.trim();

        // Silently ignore empty commands
        if cmd.is_empty() {
            return Ok(true);
        }

        // Check if user is viewing the last log before adding a new one
        let was_viewing_last_log = !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

        let result = match cmd {
            "q" | "quit" => return Ok(false),
            "w" | "write" => {
                self.logs.push("Configuration saved".to_string());
                Ok(true)
            }
            "wq" => {
                self.logs.push("Configuration saved".to_string());
                return Ok(false);
            }
            _ => {
                self.logs.push(format!("Unknown command: {}", cmd));
                Ok(true)
            }
        };

        // If user was viewing the last log, update selected_log to follow the new log
        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }

        result
    }

    /// Scroll down in the current panel
    fn scroll_down(&mut self) {
        if self.current_panel == Panel::Logs
            && self.selected_log < self.logs.len().saturating_sub(1)
        {
            self.selected_log += 1;
        }
    }

    /// Scroll up in the current panel
    fn scroll_up(&mut self) {
        if self.current_panel == Panel::Logs && self.selected_log > 0 {
            self.selected_log -= 1;
        }
    }

    /// Toggle recording state
    fn toggle_recording(&mut self) {
        // Check if user is viewing the last log before adding a new one
        let was_viewing_last_log = !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

        self.is_recording = !self.is_recording;
        if self.is_recording {
            self.logs.push("Started recording".to_string());
            self.recording_duration = 0;
            self.tick_count = 0;
        } else {
            self.logs.push("Stopped recording".to_string());
        }

        // If user was viewing the last log, update selected_log to follow the new log
        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
    }

    /// Handle a tick event
    /// Updates recording duration if currently recording
    pub fn handle_tick(&mut self) {
        if self.is_recording {
            self.tick_count += 1;
            // With 250ms tick rate, 4 ticks = 1 second
            if self.tick_count % 4 == 0 {
                self.recording_duration += 1;
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
