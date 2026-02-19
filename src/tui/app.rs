//! TUI application state and logic

use crate::config::Config;
use crate::streaming_engine::StreamingEvent;
use crate::text_filters::TextFilters;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use url::Url;

/// Actions that can be triggered by clicking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    /// Switch to a specific panel
    SwitchPanel(Panel),
    /// Toggle progressive typing
    ToggleProgressiveTyping,
    /// Toggle auto-correction
    ToggleAutoCorrection,
    /// Toggle lowercase filter
    ToggleLowercaseFilter,
    /// Toggle punctuation filter
    TogglePunctuationFilter,
    /// Toggle VAD mode
    ToggleVadMode,
    /// Select a log entry
    SelectLog(usize),
    /// Select a device from the picker (by index)
    SelectDevice(usize),
}

/// A clickable region in the UI
#[derive(Debug, Clone, Copy)]
pub struct ClickableRegion {
    /// The bounding rectangle
    pub rect: Rect,
    /// The action to perform when clicked
    pub action: ClickAction,
}

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
    /// API key for authenticated ASR services (never displayed)
    api_key: Option<String>,
    /// Log messages
    pub logs: Vec<String>,
    /// Selected log index (for scrolling)
    pub selected_log: usize,
    /// VAD mode active
    pub vad_active: bool,
    /// Whether VAD is currently detecting speech
    pub is_speaking: bool,
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
    /// Text filters for transcription output
    pub text_filters: TextFilters,
    /// Clickable regions (updated each frame)
    pub clickable_regions: Vec<ClickableRegion>,
    /// Whether the device picker is open
    pub device_picker_open: bool,
    /// Available audio devices (populated when picker opens)
    pub device_picker_devices: Vec<crate::audio::AudioDevice>,
    /// Highlighted index in the device picker
    pub device_picker_selected: usize,
    /// Error message if device list fetch failed
    pub device_picker_error: Option<String>,
    /// Active config profile name (None = default)
    pub profile: Option<String>,
    /// Available profile names (cached)
    pub available_profiles: Vec<String>,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Self {
        Self::with_profile(None)
    }

    pub fn with_profile(profile: Option<&str>) -> Self {
        // Load config from files
        let config = Config::load_profile(profile).unwrap_or_default();
        let server_url = config.whisper_server.to_string();
        let device = config.device.clone();
        let language = config.language.clone();
        let api_key = config.api_key.clone();
        let text_filters = config.text_filters.clone();

        // Use configured model if set, otherwise fetch lazily on first tick
        let model = config.model.clone().unwrap_or_else(|| "(connecting...)".to_string());

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
            api_key,
            logs: vec![
                "Application started".to_string(),
                "TUI initialized".to_string(),
            ],
            selected_log: 0,
            vad_active: false,
            is_speaking: false,
            committed_text: String::new(),
            uncommitted_text: String::new(),
            progressive_typing: true,
            auto_correction: true,
            segments_processed: 0,
            avg_latency_ms: 0,
            text_filters,
            clickable_regions: Vec::new(),
            device_picker_open: false,
            device_picker_devices: Vec::new(),
            device_picker_selected: 0,
            device_picker_error: None,
            profile: profile.map(|s| s.to_string()),
            available_profiles: Config::list_profiles().unwrap_or_default(),
        }
    }

    /// Fetch the model name from the whisper server
    fn fetch_model_name(server_url: &str, api_key: Option<&str>) -> Option<String> {
        let base = server_url.trim_end_matches('/');
        let url = format!("{}/v1/models", base);

        // Synchronous blocking request with short timeout
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .ok()?;
        let mut request = client.get(&url);
        if let Some(key) = api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().ok()?;
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

        // Handle device picker mode separately
        if self.device_picker_open {
            return self.handle_device_picker_key(key);
        }

        // Handle command mode separately
        if self.command_mode {
            return self.handle_command_key(key);
        }

        // Global keybindings
        match (key.code, key.modifiers) {
            // Quit with 'q', Escape, or Ctrl+C
            (KeyCode::Char('q'), KeyModifiers::NONE) => return Ok(false),
            (KeyCode::Esc, KeyModifiers::NONE) => return Ok(false),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(false),

            // Enter command mode with ':'
            (KeyCode::Char(':'), KeyModifiers::NONE) => {
                self.command_mode = true;
                self.command_buffer.clear();
            }

            // Tab navigation with h/l (vim-style) or Left/Right arrows
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, KeyModifiers::NONE) => {
                self.current_panel = self.current_panel.prev();
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, KeyModifiers::NONE) => {
                self.current_panel = self.current_panel.next();
            }

            // Tab navigation with Tab/Shift+Tab
            (KeyCode::Tab, KeyModifiers::NONE) => {
                self.current_panel = self.current_panel.next();
            }
            (KeyCode::BackTab, KeyModifiers::SHIFT) => {
                self.current_panel = self.current_panel.prev();
            }

            // Panel-specific navigation with j/k (vim-style) or Up/Down arrows
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                self.scroll_down();
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
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

            // 'f' to toggle lowercase filter (in Configuration panel)
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.toggle_lowercase_filter();
                }
            }

            // 'p' to toggle punctuation filter (in Configuration panel)
            (KeyCode::Char('p'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.toggle_punctuation_filter();
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

            // 'P' to cycle profile
            (KeyCode::Char('P'), KeyModifiers::SHIFT) => {
                if self.current_panel == Panel::Configuration {
                    self.cycle_profile();
                }
            }

            // 'e' to edit server URL (in Configuration panel)
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.start_editing(EditableField::ServerUrl);
                }
            }

            // 'd' to open device picker (in Configuration panel)
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.open_device_picker();
                }
            }

            _ => {}
        }

        Ok(true)
    }

    /// Handle a mouse event
    /// Returns false if the app should quit
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<bool> {
        // Only handle left clicks
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let x = mouse.column;
            let y = mouse.row;

            // Find which clickable region was clicked
            for region in &self.clickable_regions {
                if x >= region.rect.x
                    && x < region.rect.x + region.rect.width
                    && y >= region.rect.y
                    && y < region.rect.y + region.rect.height
                {
                    // Execute the action
                    match region.action {
                        ClickAction::SwitchPanel(panel) => {
                            self.current_panel = panel;
                        }
                        ClickAction::ToggleProgressiveTyping => {
                            self.toggle_progressive_typing();
                        }
                        ClickAction::ToggleAutoCorrection => {
                            self.toggle_auto_correction();
                        }
                        ClickAction::ToggleLowercaseFilter => {
                            self.toggle_lowercase_filter();
                        }
                        ClickAction::TogglePunctuationFilter => {
                            self.toggle_punctuation_filter();
                        }
                        ClickAction::ToggleVadMode => {
                            self.toggle_vad_mode();
                        }
                        ClickAction::SelectLog(index) => {
                            self.selected_log = index;
                        }
                        ClickAction::SelectDevice(index) => {
                            if self.device_picker_open && index < self.device_picker_devices.len() {
                                self.device_picker_selected = index;
                                self.confirm_device_selection();
                            }
                        }
                    }
                    break;
                }
            }
        }

        Ok(true)
    }

    /// Clear clickable regions (called before each render)
    pub fn clear_clickable_regions(&mut self) {
        self.clickable_regions.clear();
    }

    /// Add a clickable region
    pub fn add_clickable_region(&mut self, rect: Rect, action: ClickAction) {
        self.clickable_regions
            .push(ClickableRegion { rect, action });
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
                            self.save_config();
                            self.logs
                                .push(format!("Server URL set to: {}", self.server));
                            // Update model from new server
                            if let Some(model) = Self::fetch_model_name(&self.server, self.api_key.as_deref()) {
                                self.model = model;
                                self.logs.push(format!("Model updated: {}", self.model));
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
        if self.vad_active {
            self.add_log("Cannot record while VAD is active");
            return;
        }

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

        self.save_config();

        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
    }

    /// Cycle through available profiles: default -> profile1 -> profile2 -> ... -> default
    fn cycle_profile(&mut self) {
        if self.available_profiles.is_empty() {
            self.add_log("No named profiles found");
            return;
        }

        let current = self.profile.as_deref();
        let next = match current {
            None => Some(self.available_profiles[0].as_str()),
            Some(name) => {
                let idx = self.available_profiles.iter().position(|p| p == name);
                match idx {
                    Some(i) if i + 1 < self.available_profiles.len() => {
                        Some(self.available_profiles[i + 1].as_str())
                    }
                    _ => None, // wrap to default
                }
            }
        };

        let profile_name = next.map(|s| s.to_string());
        let display = profile_name.as_deref().unwrap_or("default").to_string();

        // Persist the choice
        Config::set_default_profile(profile_name.as_deref().unwrap_or("")).ok();

        // Reload config from the new profile
        let config = Config::load_profile(profile_name.as_deref()).unwrap_or_default();
        self.server = config.whisper_server.to_string();
        self.device = config.device.clone();
        self.language = config.language.clone();
        self.api_key = config.api_key.clone();
        self.model = config.model.clone().unwrap_or_else(|| "(connecting...)".to_string());
        self.text_filters = config.text_filters.clone();
        self.profile = profile_name;

        self.add_log(&format!("Profile switched to: {}", display));
    }

    /// Toggle VAD mode
    pub fn toggle_vad_mode(&mut self) {
        if self.is_recording {
            self.add_log("Cannot enable VAD while recording");
            return;
        }

        self.vad_active = !self.vad_active;
        if self.vad_active {
            self.add_log("VAD mode enabled");
            // Reset streaming state
            self.committed_text.clear();
            self.uncommitted_text.clear();
            self.segments_processed = 0;
            self.is_speaking = false;
        } else {
            self.add_log("VAD mode disabled");
            self.is_speaking = false;
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

    /// Toggle lowercase filter
    pub fn toggle_lowercase_filter(&mut self) {
        self.text_filters.lowercase = !self.text_filters.lowercase;
        let status = if self.text_filters.lowercase {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Lowercase filter {}", status));
        self.save_config();
    }

    /// Toggle punctuation removal filter
    pub fn toggle_punctuation_filter(&mut self) {
        self.text_filters.remove_punctuation = !self.text_filters.remove_punctuation;
        let status = if self.text_filters.remove_punctuation {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Punctuation filter {}", status));
        self.save_config();
    }

    /// Save current settings to config.toml
    fn save_config(&self) {
        // Reconstruct Config from App fields and save as TOML
        let mut config = Config::load_profile(self.profile.as_deref()).unwrap_or_default();
        if let Ok(url) = Url::parse(&self.server) {
            config.whisper_server = url;
        }
        config.device = self.device.clone();
        config.language = self.language.clone();
        config.text_filters = self.text_filters.clone();
        if let Err(e) = config.save() {
            tracing::warn!("Failed to save config: {}", e);
        }
    }

    /// Open the device picker
    fn open_device_picker(&mut self) {
        match crate::audio::list_devices() {
            Ok(devices) => {
                if devices.is_empty() {
                    self.device_picker_error = Some("No audio input devices found".to_string());
                } else {
                    self.device_picker_error = None;
                }

                let current_index = devices
                    .iter()
                    .position(|d| d.name == self.device)
                    .unwrap_or(0);

                self.device_picker_devices = devices;
                self.device_picker_selected = current_index;
                self.device_picker_open = true;
            }
            Err(e) => {
                let msg = format!("Failed to list devices: {}", e);
                self.logs.push(msg.clone());
                self.device_picker_error = Some(msg);
                self.device_picker_devices = Vec::new();
                self.device_picker_open = true;
            }
        }
    }

    /// Close the device picker without selecting
    fn close_device_picker(&mut self) {
        self.device_picker_open = false;
        self.device_picker_devices.clear();
        self.device_picker_error = None;
    }

    /// Confirm device selection and save to config
    fn confirm_device_selection(&mut self) {
        if self.device_picker_devices.is_empty() {
            self.close_device_picker();
            return;
        }

        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;

        let selected = &self.device_picker_devices[self.device_picker_selected];
        let new_name = selected.name.clone();
        let description = selected.description.clone();

        self.device = new_name.clone();
        self.save_config();
        self.logs.push(format!("Device set to: {}", description));

        self.close_device_picker();

        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
    }

    /// Handle key press in device picker mode
    fn handle_device_picker_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.close_device_picker();
            }
            KeyCode::Enter => {
                self.confirm_device_selection();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.device_picker_devices.is_empty() {
                    self.device_picker_selected =
                        (self.device_picker_selected + 1) % self.device_picker_devices.len();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.device_picker_devices.is_empty() {
                    self.device_picker_selected = if self.device_picker_selected == 0 {
                        self.device_picker_devices.len() - 1
                    } else {
                        self.device_picker_selected - 1
                    };
                }
            }
            _ => {}
        }
        Ok(true)
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

    /// Add a log message, auto-scrolling if viewing the last entry
    pub fn add_log(&mut self, msg: &str) {
        let was_viewing_last_log =
            !self.logs.is_empty() && self.selected_log == self.logs.len() - 1;
        self.logs.push(msg.to_string());
        if was_viewing_last_log {
            self.selected_log = self.logs.len() - 1;
        }
    }

    /// Handle a streaming event from the VAD pipeline
    pub fn handle_streaming_event(&mut self, event: StreamingEvent) {
        match event {
            StreamingEvent::SpeechStarted => {
                self.is_speaking = true;
            }
            StreamingEvent::SpeechEnded => {
                self.is_speaking = false;
            }
            StreamingEvent::TranscriptUpdate {
                committed,
                uncommitted,
            } => {
                self.committed_text = committed;
                self.uncommitted_text = uncommitted;
            }
            StreamingEvent::SegmentCompleted { text, duration_ms } => {
                self.add_log(&format!("Segment: \"{}\" ({}ms)", text, duration_ms));
            }
            StreamingEvent::StatsUpdate {
                segments_processed,
                avg_latency_ms,
            } => {
                self.segments_processed = segments_processed;
                self.avg_latency_ms = avg_latency_ms;
            }
            StreamingEvent::Error(msg) => {
                self.add_log(&format!("Streaming error: {}", msg));
            }
        }
    }

    /// Handle a tick event
    /// Updates recording duration if currently recording
    pub fn handle_tick(&mut self) {
        self.tick_count += 1;

        // Lazy fetch model on first tick, only if not already configured
        if self.tick_count == 1 && self.model == "(connecting...)" {
            self.model =
                Self::fetch_model_name(&self.server, self.api_key.as_deref()).unwrap_or_else(|| "(offline)".to_string());
        }

        if self.is_recording {
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
