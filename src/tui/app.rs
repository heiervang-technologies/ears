//! TUI application state and logic

use crate::config::Config;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use url::Url;

/// The current panel being displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Status,
    Configuration,
    Logs,
    LiveTranscription,
}

/// Editable configuration fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditableField {
    ServerUrl,
}

impl Panel {
    /// Get the next panel (tab right)
    pub fn next(self) -> Self {
        match self {
            Panel::Status => Panel::Configuration,
            Panel::Configuration => Panel::Logs,
            Panel::Logs => Panel::LiveTranscription,
            Panel::LiveTranscription => Panel::Status,
        }
    }

    /// Get the previous panel (tab left)
    pub fn prev(self) -> Self {
        match self {
            Panel::Status => Panel::LiveTranscription,
            Panel::Configuration => Panel::Status,
            Panel::Logs => Panel::Configuration,
            Panel::LiveTranscription => Panel::Logs,
        }
    }

    /// Get the panel title
    pub fn title(self) -> &'static str {
        match self {
            Panel::Status => "Status",
            Panel::Configuration => "Configuration",
            Panel::Logs => "Logs",
            Panel::LiveTranscription => "Live",
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
    /// Currently editing field (None = not editing)
    pub editing_field: Option<EditableField>,
    /// Buffer for the field being edited
    pub edit_buffer: String,
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
    /// Language for transcription (None = auto-detect)
    pub language: Option<String>,
    /// Config directory path
    config_dir: std::path::PathBuf,
    /// Log messages
    pub logs: Vec<String>,
    /// Selected log index (for scrolling)
    pub selected_log: usize,
    /// VAD mode active
    pub vad_active: bool,
    /// Committed (stable) transcription text
    pub committed_text: String,
    /// Uncommitted (unstable) transcription text
    pub uncommitted_text: String,
    /// Progressive typing enabled
    pub progressive_typing: bool,
    /// Auto-correction enabled
    pub auto_correction: bool,
    /// Number of segments processed (for stats)
    pub segments_processed: usize,
    /// Average latency in milliseconds (for stats)
    pub avg_latency_ms: u64,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Self {
        // Load config from files
        let config = Config::load().unwrap_or_default();
        let server_url = config.whisper_server.to_string();
        let device = config.device.clone();
        let language = config.language.clone();
        let config_dir = config.config_dir.clone();

        // Try to fetch model from the server's /v1/models endpoint
        let model = Self::fetch_model_name(&server_url).unwrap_or_else(|| "unknown".to_string());

        Self {
            current_panel: Panel::Status,
            command_mode: false,
            command_buffer: String::new(),
            editing_field: None,
            edit_buffer: String::new(),
            is_recording: false,
            recording_duration: 0,
            tick_count: 0,
            model,
            server: server_url,
            device,
            language,
            config_dir,
            logs: vec![
                "Application started".to_string(),
                "TUI initialized".to_string(),
            ],
            selected_log: 0,
            vad_active: false,
            committed_text: String::new(),
            uncommitted_text: String::new(),
            progressive_typing: true,
            auto_correction: true,
            segments_processed: 0,
            avg_latency_ms: 0,
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
        // Handle edit mode separately
        if self.editing_field.is_some() {
            return self.handle_edit_key(key);
        }

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

            // Space to toggle recording (in Status panel) or VAD mode (in Live panel)
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                if self.current_panel == Panel::LiveTranscription {
                    self.toggle_vad_mode();
                } else {
                    self.toggle_recording();
                }
            }

            // 'v' to toggle VAD mode
            (KeyCode::Char('v'), KeyModifiers::NONE) => {
                self.toggle_vad_mode();
            }

            // 't' to toggle progressive typing (in Live panel)
            (KeyCode::Char('t'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::LiveTranscription {
                    self.toggle_progressive_typing();
                }
            }

            // 'a' to toggle auto-correction (in Live panel)
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::LiveTranscription {
                    self.toggle_auto_correction();
                }
            }

            // 'c' to go to configuration panel
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.current_panel = Panel::Configuration;
            }

            // 'L' to cycle language (auto -> en -> no -> auto)
            (KeyCode::Char('L'), KeyModifiers::SHIFT) => {
                self.cycle_language();
            }

            // 'e' to edit server URL (in Configuration panel)
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.start_editing(EditableField::ServerUrl);
                }
            }

            _ => {}
        }

        Ok(true)
    }

    /// Start editing a field
    fn start_editing(&mut self, field: EditableField) {
        self.editing_field = Some(field);
        self.edit_buffer = match field {
            EditableField::ServerUrl => self.server.clone(),
        };
    }

    /// Handle key press in edit mode
    fn handle_edit_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                // Cancel editing
                self.editing_field = None;
                self.edit_buffer.clear();
            }
            KeyCode::Enter => {
                // Save the edit
                self.save_edit();
            }
            KeyCode::Char(c) => {
                self.edit_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            _ => {}
        }
        Ok(true)
    }

    /// Save the current edit
    fn save_edit(&mut self) {
        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

        if let Some(field) = self.editing_field {
            match field {
                EditableField::ServerUrl => {
                    // Validate URL
                    match Url::parse(&self.edit_buffer) {
                        Ok(url) => {
                            self.server = url.to_string();
                            // Save to config file
                            let server_file = self.config_dir.join("server");
                            if let Err(e) = std::fs::write(&server_file, &self.server) {
                                self.logs.push(format!("Failed to save server URL: {}", e));
                            } else {
                                self.logs.push(format!("Server URL set to: {}", self.server));
                                // Update model from new server
                                if let Some(model) = Self::fetch_model_name(&self.server) {
                                    self.model = model;
                                    self.logs.push(format!("Model updated: {}", self.model));
                                }
                            }
                        }
                        Err(e) => {
                            self.logs.push(format!("Invalid URL: {}", e));
                        }
                    }
                }
            }
        }

        self.editing_field = None;
        self.edit_buffer.clear();

        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
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
        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

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
        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

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

    /// Cycle through languages: auto -> en -> no -> auto
    fn cycle_language(&mut self) {
        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

        self.language = match &self.language {
            None => Some("en".to_string()),
            Some(lang) if lang == "en" => Some("no".to_string()),
            Some(_) => None,
        };

        let lang_display = self.language.as_deref().unwrap_or("auto");
        self.logs.push(format!("Language set to: {}", lang_display));

        // Save to config file
        let language_file = self.config_dir.join("language");
        let content = self.language.as_deref().unwrap_or("");
        if let Err(e) = std::fs::write(&language_file, content) {
            self.logs.push(format!("Failed to save language: {}", e));
        }

        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
    }

    /// Toggle VAD mode
    pub fn toggle_vad_mode(&mut self) {
        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

        self.vad_active = !self.vad_active;
        if self.vad_active {
            self.logs.push("VAD mode enabled".to_string());
            // Reset streaming state
            self.committed_text.clear();
            self.uncommitted_text.clear();
            self.segments_processed = 0;
        } else {
            self.logs.push("VAD mode disabled".to_string());
        }

        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
    }

    /// Toggle progressive typing setting
    pub fn toggle_progressive_typing(&mut self) {
        self.progressive_typing = !self.progressive_typing;
        let status = if self.progressive_typing {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Progressive typing {}", status));
    }

    /// Toggle auto-correction setting
    pub fn toggle_auto_correction(&mut self) {
        self.auto_correction = !self.auto_correction;
        let status = if self.auto_correction {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Auto-correction {}", status));
    }

    /// Update streaming transcription state (called from streaming engine)
    pub fn update_streaming_state(
        &mut self,
        committed: String,
        uncommitted: String,
        segments_processed: usize,
        avg_latency_ms: u64,
    ) {
        self.committed_text = committed;
        self.uncommitted_text = uncommitted;
        self.segments_processed = segments_processed;
        self.avg_latency_ms = avg_latency_ms;
    }

    /// Handle a tick event
    /// Updates recording duration if currently recording
    pub fn handle_tick(&mut self) {
        if self.is_recording {
            self.tick_count += 1;
            // With 250ms tick rate, 4 ticks = 1 second
            if self.tick_count.is_multiple_of(4) {
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
