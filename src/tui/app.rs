//! TUI application state and logic

use super::theme::{Theme, ThemeName};
use crate::config::Config;
use crate::desktop::TypingMode;
use crate::ducker::VolumeDucker;
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
    /// Toggle strict alphabet filter
    ToggleStrictAlphabetFilter,
    /// Toggle auto-enter
    ToggleAutoEnter,
    /// Toggle save to clipboard
    ToggleSaveToClipboard,
    /// Toggle VAD mode
    ToggleVadMode,
    /// Toggle volume ducking
    ToggleDuck,
    /// Set duck percent (0-100) — used by slider clicks
    SetDuckPercent(u8),
    /// Select a log entry
    SelectLog(usize),
    /// Cycle typing mode
    CycleTypingMode,
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

/// Log filter level for the Logs panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFilter {
    All,
    Errors,
    Warnings,
}

impl LogFilter {
    /// Cycle to the next filter
    pub fn next(self) -> Self {
        match self {
            LogFilter::All => LogFilter::Errors,
            LogFilter::Errors => LogFilter::Warnings,
            LogFilter::Warnings => LogFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LogFilter::All => "All",
            LogFilter::Errors => "Errors",
            LogFilter::Warnings => "Warnings",
        }
    }

    /// Check if a log message passes this filter
    pub fn matches(self, msg: &str) -> bool {
        match self {
            LogFilter::All => true,
            LogFilter::Errors => {
                let lower = msg.to_lowercase();
                lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("cannot")
                    || lower.contains("invalid")
            }
            LogFilter::Warnings => {
                let lower = msg.to_lowercase();
                lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("cannot")
                    || lower.contains("invalid")
                    || lower.contains("warning")
                    || lower.contains("offline")
            }
        }
    }
}

/// The current panel being displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
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
            Panel::Configuration => Panel::Logs,
            Panel::Logs => Panel::LiveTranscription,
            Panel::LiveTranscription => Panel::Configuration,
        }
    }

    /// Get the previous panel (tab left)
    pub fn prev(self) -> Self {
        match self {
            Panel::Configuration => Panel::LiveTranscription,
            Panel::Logs => Panel::Configuration,
            Panel::LiveTranscription => Panel::Logs,
        }
    }

    /// Get the panel title
    pub fn title(self) -> &'static str {
        match self {
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
    /// Tick counter for lazy model fetch
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
    /// VAD mode active (managed by this TUI instance)
    pub vad_active: bool,
    /// External VAD process is running (display only, not managed by TUI)
    pub external_vad_active: bool,
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
    /// Send Enter key after each transcription
    pub auto_enter: bool,
    /// Save transcribed text to clipboard
    pub save_to_clipboard: bool,
    /// Volume ducking enabled (lowers system volume during speech)
    pub duck_enabled: bool,
    /// Volume ducking percent (0-100; 50 = halve current volume)
    pub duck_percent: u8,
    /// Volume ducker — drives wpctl in response to VAD events
    pub ducker: VolumeDucker,
    /// Bash mode: constrain ASR output to a shell grammar (constrained decoding)
    pub bash_mode: bool,
    /// Custom guided grammar override (None = use built-in bash grammar)
    pub guided_grammar: Option<String>,
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
    /// Audio cue volume (0-100)
    pub cue_volume: u8,
    /// Text input method (auto/wtype/paste)
    pub typing_mode: TypingMode,
    /// Active config profile name (None = default)
    pub profile: Option<String>,
    /// Available profile names (cached)
    pub available_profiles: Vec<String>,
    /// Active log filter
    pub log_filter: LogFilter,
    /// Whether the help overlay is open
    pub help_overlay_open: bool,
    /// Whether log search mode is active
    pub search_mode: bool,
    /// Current search query buffer
    pub search_buffer: String,
    /// Indices of log entries matching the search
    pub search_matches: Vec<usize>,
    /// Current position within search_matches
    pub search_match_index: usize,
    /// Whether server URL is set via EARS_SERVER env var
    pub env_server: bool,
    /// Whether device is set via EARS_DEVICE env var
    pub env_device: bool,
    /// Whether language is set via EARS_LANGUAGE env var
    pub env_language: bool,
    /// Whether model is set via EARS_MODEL env var
    pub env_model: bool,
    /// Total transcription attempts (success + failure)
    pub total_transcriptions: usize,
    /// Successful transcription count
    pub successful_transcriptions: usize,
    /// Failed transcription count
    pub failed_transcriptions: usize,
    /// Total words transcribed
    pub total_words: usize,
    /// Current theme name
    pub theme_name: ThemeName,
    /// Current theme colors
    pub theme: Theme,
    /// Event sender to queue UI events from background tasks
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tui::Event>>,
}

impl App {
    /// Create a new application instance
    pub fn new() -> Self {
        Self::with_profile(None)
    }

    pub fn with_profile(profile: Option<&str>) -> Self {
        // Load config from files
        let config = Config::load_profile(profile).unwrap_or_else(|_| Config::new().expect("Failed to create default config"));
        let server_url = config.whisper_server.to_string();
        let device = config.device.clone();
        let language = config.language.clone();
        let api_key = config.api_key.clone();
        let text_filters = config.text_filters.clone();
        let typing_mode = config.typing_mode;
        let auto_enter = config.auto_enter;
        let progressive_typing = config.progressive_typing;
        let auto_correction = config.effective_auto_correction();
        let cue_volume = config.cue_volume;
        let bash_mode = config.bash_mode;
        let guided_grammar = config.guided_grammar.clone();
        let active_profile = config.active_profile.clone();
        let duck_enabled = config.vad.duck_enabled;
        let duck_percent = config.vad.duck_percent;
        let ducker = VolumeDucker::new(duck_enabled, duck_percent);

        // Set global audio volume from config
        crate::desktop::AudioFeedback::set_volume(cue_volume);

        // Use configured model if set, otherwise fetch lazily on first tick
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "(connecting...)".to_string());

        Self {
            current_panel: Panel::Configuration,
            command_mode: false,
            command_buffer: String::new(),
            editing_field: None,
            edit_buffer: String::new(),
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
            external_vad_active: crate::state::is_external_vad_alive(&config.state_dir),
            is_speaking: false,
            committed_text: String::new(),
            uncommitted_text: String::new(),
            progressive_typing,
            auto_correction,
            auto_enter,
            save_to_clipboard: config.save_to_clipboard,
            duck_enabled,
            duck_percent,
            ducker,
            bash_mode,
            guided_grammar,
            segments_processed: 0,
            avg_latency_ms: 0,
            text_filters,
            cue_volume,
            typing_mode,
            clickable_regions: Vec::new(),
            device_picker_open: false,
            device_picker_devices: Vec::new(),
            device_picker_selected: 0,
            device_picker_error: None,
            profile: active_profile,
            available_profiles: Config::list_profiles().unwrap_or_default(),
            log_filter: LogFilter::All,
            help_overlay_open: false,
            search_mode: false,
            search_buffer: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            env_server: std::env::var("EARS_SERVER").is_ok(),
            env_device: std::env::var("EARS_DEVICE").is_ok(),
            env_language: std::env::var("EARS_LANGUAGE").is_ok(),
            env_model: std::env::var("EARS_MODEL").is_ok(),
            total_transcriptions: 0,
            successful_transcriptions: 0,
            failed_transcriptions: 0,
            total_words: 0,
            theme_name: ThemeName::Dark,
            theme: Theme::dark(),
            event_tx: None,
        }
    }

    /// Spawn a task to fetch the model name asynchronously
    pub fn trigger_model_fetch(&self) {
        if let Some(tx) = &self.event_tx {
            let tx = tx.clone();
            let server_url = self.server.clone();
            let api_key = self.api_key.clone();

            tokio::spawn(async move {
                let base = server_url.trim_end_matches('/');
                let url = format!("{}/v1/models", base);

                let client = match reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(2))
                    .timeout(std::time::Duration::from_secs(3))
                    .build()
                {
                    Ok(c) => c,
                    Err(_) => {
                        let _ = tx.send(crate::tui::Event::ModelFetched(None));
                        return;
                    }
                };

                let mut request = client.get(&url);
                if let Some(key) = api_key {
                    request = request.bearer_auth(key);
                }

                let model_name = async {
                    let response = request.send().await.ok()?;
                    let json: serde_json::Value = response.json().await.ok()?;
                    json.get("data")?
                        .as_array()?
                        .first()?
                        .get("id")?
                        .as_str()
                        .map(|s| s.to_string())
                }
                .await;

                let _ = tx.send(crate::tui::Event::ModelFetched(model_name));
            });
        }
    }

    /// Handle a key press event
    /// Returns false if the app should quit
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Handle help overlay — absorbs all keys except ? and Esc (which close it)
        if self.help_overlay_open {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => {
                    self.help_overlay_open = false;
                }
                _ => {}
            }
            return Ok(true);
        }

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

        // Handle search mode separately
        if self.search_mode {
            return self.handle_search_key(key);
        }

        // Global keybindings
        match (key.code, key.modifiers) {
            // Quit with 'q', Escape, or Ctrl+C
            (KeyCode::Char('q'), KeyModifiers::NONE) => return Ok(false),
            (KeyCode::Esc, KeyModifiers::NONE) => return Ok(false),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(false),

            // Toggle help overlay with '?'
            (KeyCode::Char('?'), KeyModifiers::NONE) => {
                self.help_overlay_open = true;
            }

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

            // Space to toggle VAD mode (consistent across all panels)
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                self.toggle_vad_mode();
            }

            // 'v' to toggle VAD mode
            (KeyCode::Char('v'), KeyModifiers::NONE) => {
                self.toggle_vad_mode();
            }

            // 't' to toggle progressive typing (in Live/Configuration panels)
            (KeyCode::Char('t'), KeyModifiers::NONE)
                if (self.current_panel == Panel::LiveTranscription
                    || self.current_panel == Panel::Configuration) =>
            {
                self.toggle_progressive_typing();
            }

            // 'a' to toggle auto-correction (in Live/Configuration panels)
            (KeyCode::Char('a'), KeyModifiers::NONE)
                if (self.current_panel == Panel::LiveTranscription
                    || self.current_panel == Panel::Configuration) =>
            {
                self.toggle_auto_correction();
            }

            // 'f' to toggle lowercase filter (in Configuration panel)
            (KeyCode::Char('f'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.toggle_lowercase_filter();
            }

            // 'p' to toggle punctuation filter (in Configuration panel)
            (KeyCode::Char('p'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.toggle_punctuation_filter();
            }

            // 's' to toggle strict alphabet filter (in Configuration panel)
            (KeyCode::Char('s'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.toggle_strict_alphabet_filter();
            }

            // 'm' to cycle typing mode (in Configuration panel)
            (KeyCode::Char('m'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.cycle_typing_mode();
            }

            // 'g' to toggle bash mode (constrained decoding) in Configuration panel
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.toggle_bash_mode();
                }
            }

            // '+' / '=' to increase cue volume, '-' to decrease (in Configuration panel)
            (KeyCode::Char('+') | KeyCode::Char('='), _)
                if self.current_panel == Panel::Configuration =>
            {
                self.adjust_cue_volume(10);
            }
            (KeyCode::Char('-'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.adjust_cue_volume(-10);
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
            (KeyCode::Char('P'), KeyModifiers::SHIFT)
                if self.current_panel == Panel::Configuration =>
            {
                self.cycle_profile();
            }

            // 'e' to edit server URL (in Configuration panel)
            (KeyCode::Char('e'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.start_editing(EditableField::ServerUrl);
            }

            // 'd' to open device picker (in Configuration panel)
            (KeyCode::Char('d'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.open_device_picker();
            }

            // 'F' to cycle log filter (in Logs panel)
            (KeyCode::Char('F'), KeyModifiers::SHIFT) if self.current_panel == Panel::Logs => {
                self.log_filter = self.log_filter.next();
                self.add_log(&format!("Log filter: {}", self.log_filter.label()));
            }

            // '/' to search logs (in Logs panel)
            (KeyCode::Char('/'), KeyModifiers::NONE) if self.current_panel == Panel::Logs => {
                self.search_mode = true;
                self.search_buffer.clear();
                self.search_matches.clear();
                self.search_match_index = 0;
            }

            // 'n' to jump to next search match (Logs) or toggle auto-enter (Configuration)
            (KeyCode::Char('n'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Logs && !self.search_matches.is_empty() {
                    self.search_match_index =
                        (self.search_match_index + 1) % self.search_matches.len();
                    self.selected_log = self.search_matches[self.search_match_index];
                } else if self.current_panel == Panel::Configuration {
                    self.toggle_auto_enter();
                }
            }

            // 'b' to toggle save to clipboard (in Configuration panel)
            (KeyCode::Char('b'), KeyModifiers::NONE)
                if self.current_panel == Panel::Configuration =>
            {
                self.toggle_save_to_clipboard();
            }

            // Shift+D to toggle volume ducking (works from any panel)
            (KeyCode::Char('D'), KeyModifiers::SHIFT) => {
                self.toggle_duck();
            }

            // '[' / ']' to nudge duck percent (in Configuration panel)
            (KeyCode::Char('['), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.adjust_duck_percent(-5);
                }
            }
            (KeyCode::Char(']'), KeyModifiers::NONE) => {
                if self.current_panel == Panel::Configuration {
                    self.adjust_duck_percent(5);
                }
            }

            // 'N' to jump to previous search match
            (KeyCode::Char('N'), KeyModifiers::SHIFT)
                if self.current_panel == Panel::Logs && !self.search_matches.is_empty() =>
            {
                self.search_match_index = if self.search_match_index == 0 {
                    self.search_matches.len() - 1
                } else {
                    self.search_match_index - 1
                };
                self.selected_log = self.search_matches[self.search_match_index];
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
                        ClickAction::ToggleStrictAlphabetFilter => {
                            self.toggle_strict_alphabet_filter();
                        }
                        ClickAction::ToggleAutoEnter => {
                            self.toggle_auto_enter();
                        }
                        ClickAction::ToggleSaveToClipboard => {
                            self.toggle_save_to_clipboard();
                        }
                        ClickAction::ToggleVadMode => {
                            self.toggle_vad_mode();
                        }
                        ClickAction::ToggleDuck => {
                            self.toggle_duck();
                        }
                        ClickAction::SetDuckPercent(p) => {
                            self.set_duck_percent(p);
                        }
                        ClickAction::CycleTypingMode => {
                            self.cycle_typing_mode();
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
        // Block editing if field is set via env var
        match field {
            EditableField::ServerUrl => {
                if self.env_server {
                    self.add_log("Cannot edit: set via EARS_SERVER env var");
                    return;
                }
            }
        }
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
                            self.model = "(connecting...)".to_string();
                            self.trigger_model_fetch();
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

    /// Handle key press in search mode
    fn handle_search_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.search_mode = false;
                self.search_buffer.clear();
                self.search_matches.clear();
                self.search_match_index = 0;
            }
            KeyCode::Enter => {
                self.search_mode = false;
                if !self.search_matches.is_empty() {
                    self.search_match_index = 0;
                    self.selected_log = self.search_matches[0];
                }
            }
            KeyCode::Char(c) => {
                self.search_buffer.push(c);
                self.update_search_matches();
            }
            KeyCode::Backspace => {
                self.search_buffer.pop();
                self.update_search_matches();
            }
            _ => {}
        }
        Ok(true)
    }

    /// Recalculate search matches based on search_buffer
    fn update_search_matches(&mut self) {
        self.search_matches.clear();
        self.search_match_index = 0;
        if !self.search_buffer.is_empty() {
            let query = self.search_buffer.to_lowercase();
            for (i, log) in self.logs.iter().enumerate() {
                if log.to_lowercase().contains(&query) {
                    self.search_matches.push(i);
                }
            }
        }
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
            _ if cmd == "export" || cmd.starts_with("export ") => {
                let cmd_owned = cmd.to_string();
                self.export_logs(&cmd_owned);
                Ok(true)
            }
            "theme" => {
                // Toggle theme
                self.theme_name = self.theme_name.next();
                self.theme = Theme::from_name(self.theme_name);
                self.logs
                    .push(format!("Theme: {}", self.theme_name.label()));
                Ok(true)
            }
            _ if cmd.starts_with("theme ") => {
                let name = cmd.strip_prefix("theme ").unwrap_or("").trim();
                match ThemeName::parse(name) {
                    Some(t) => {
                        self.theme_name = t;
                        self.theme = Theme::from_name(t);
                        self.logs.push(format!("Theme: {}", t.label()));
                    }
                    None => {
                        self.logs
                            .push(format!("Unknown theme: {} (available: dark, light)", name));
                    }
                }
                Ok(true)
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

    /// Export logs to a file
    fn export_logs(&mut self, cmd: &str) {
        use std::fs;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let path = if cmd == "export" {
            // Default path: ~/.local/share/ears/logs/ears-{timestamp}.log
            let secs = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let dir = directories::ProjectDirs::from("com", "heiervang", "ears")
                .map(|p| p.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
                .join("logs");
            dir.join(format!("ears-{}.log", secs))
        } else {
            // Custom path: :export /path/to/file
            let path_str = cmd.strip_prefix("export ").unwrap_or("").trim();
            PathBuf::from(path_str)
        };

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                self.logs.push(format!("Export failed (mkdir): {}", e));
                return;
            }
        }

        let entry_count = self.logs.len();
        let content = self.logs.join("\n");
        match fs::write(&path, &content) {
            Ok(()) => {
                self.logs.push(format!(
                    "Logs exported to: {} ({} entries)",
                    path.display(),
                    entry_count
                ));
            }
            Err(e) => {
                self.logs.push(format!("Export failed: {}", e));
            }
        }
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
        let config = Config::load_profile(profile_name.as_deref()).unwrap_or_else(|_| Config::new().expect("Failed to create default config"));
        self.server = config.whisper_server.to_string();
        self.device = config.device.clone();
        self.language = config.language.clone();
        self.api_key = config.api_key.clone();
        self.model = config
            .model
            .clone()
            .unwrap_or_else(|| "(connecting...)".to_string());
        self.text_filters = config.text_filters.clone();
        self.typing_mode = config.typing_mode;
        self.progressive_typing = config.progressive_typing;
        self.save_to_clipboard = config.save_to_clipboard;
        self.auto_correction = config.effective_auto_correction();
        self.auto_enter = config.auto_enter;
        self.duck_enabled = config.vad.duck_enabled;
        self.duck_percent = config.vad.duck_percent;
        self.ducker
            .set_settings(self.duck_enabled, self.duck_percent);
        self.profile = profile_name;

        self.add_log(&format!("Profile switched to: {}", display));
    }

    /// Toggle VAD mode
    pub fn toggle_vad_mode(&mut self) {
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
            // Restore audio volume if we were ducking mid-speech.
            self.ducker.on_speech_ended();
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
        self.save_config();
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
        self.save_config();
    }

    /// Toggle save to clipboard setting
    pub fn toggle_save_to_clipboard(&mut self) {
        self.save_to_clipboard = !self.save_to_clipboard;
        let status = if self.save_to_clipboard {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Save to clipboard {}", status));
        self.save_config();
    }

    /// Cycle typing mode: Auto -> Wtype -> Paste -> Auto
    pub fn cycle_typing_mode(&mut self) {
        self.typing_mode = self.typing_mode.next();
        self.add_log(&format!("Typing mode: {}", self.typing_mode.display_name()));
        self.save_config();
    }

    /// Toggle volume ducking
    pub fn toggle_duck(&mut self) {
        self.duck_enabled = !self.duck_enabled;
        self.ducker
            .set_settings(self.duck_enabled, self.duck_percent);
        // If turning off mid-speech, restore immediately.
        if !self.duck_enabled {
            self.ducker.on_speech_ended();
        }
        let status = if self.duck_enabled {
            "enabled"
        } else {
            "disabled"
        };
        self.add_log(&format!("Volume ducking {}", status));
        self.save_config();
    }

    /// Set duck percent directly (clamped 0-100). Used by slider clicks.
    pub fn set_duck_percent(&mut self, percent: u8) {
        let clamped = percent.min(100);
        if clamped == self.duck_percent {
            return;
        }
        self.duck_percent = clamped;
        self.ducker
            .set_settings(self.duck_enabled, self.duck_percent);
        self.add_log(&format!("Duck percent: {}%", self.duck_percent));
        self.save_config();
    }

    /// Adjust duck percent by delta (clamped 0-100). Used by [/] keys.
    pub fn adjust_duck_percent(&mut self, delta: i16) {
        let new_pct = (self.duck_percent as i16 + delta).clamp(0, 100) as u8;
        self.set_duck_percent(new_pct);
    }

    /// Adjust cue volume by delta (clamped to 0-100)
    pub fn adjust_cue_volume(&mut self, delta: i16) {
        let new_vol = (self.cue_volume as i16 + delta).clamp(0, 100) as u8;
        self.cue_volume = new_vol;
        crate::desktop::AudioFeedback::set_volume(new_vol);
        self.add_log(&format!("Cue volume: {}%", new_vol));
        // Play a preview beep so user can hear the new level
        crate::desktop::AudioFeedback::beep_start().ok();
        self.save_config();
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

    /// Toggle strict alphabet filter
    pub fn toggle_strict_alphabet_filter(&mut self) {
        self.text_filters.strict_alphabet = !self.text_filters.strict_alphabet;
        let status = if self.text_filters.strict_alphabet {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Strict alphabet filter {}", status));
        self.save_config();
    }

    /// Resolve the active guided grammar (bash mode). Mirrors
    /// [`crate::config::Config::active_grammar`].
    pub fn active_grammar(&self) -> Option<String> {
        if !self.bash_mode {
            return None;
        }
        Some(
            self.guided_grammar
                .clone()
                .unwrap_or_else(|| crate::config::Config::BASH_GRAMMAR.to_string()),
        )
    }

    /// Toggle bash mode (constrained decoding to shell grammar)
    pub fn toggle_bash_mode(&mut self) {
        self.bash_mode = !self.bash_mode;
        let status = if self.bash_mode { "on" } else { "off" };
        self.add_log(&format!("Bash mode: {}", status));
        self.save_config();
    }

    /// Toggle auto-enter (send Enter key after each transcription)
    pub fn toggle_auto_enter(&mut self) {
        self.auto_enter = !self.auto_enter;
        let status = if self.auto_enter {
            "enabled"
        } else {
            "disabled"
        };
        self.logs.push(format!("Auto-enter {}", status));
        self.save_config();
    }

    /// Save current settings to the active config file
    fn save_config(&self) {
        // Never persist during unit tests: the TUI tests construct a real `App`
        // (which reads the user's actual ~/.config/ears) and exercise toggle
        // keys, which would otherwise overwrite the developer's real config.
        if cfg!(test) {
            return;
        }
        // Load the existing config first so fields the TUI doesn't manage
        // (model, prompt, language_servers, vad, ...) are preserved on save.
        //
        // CRITICAL: do NOT fall back to a default config if the load fails.
        // Writing defaults here would clobber the user's real settings (server,
        // model, device) — e.g. if `apply_env_overrides` transiently errors on a
        // bad EARS_* env var. On load failure, skip the save entirely.
        let mut config = match Config::load_profile(self.profile.as_deref()) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(
                    "Skipping config save (could not load existing config): {}",
                    e
                );
                return;
            }
        };
        if let Ok(url) = Url::parse(&self.server) {
            config.whisper_server = url;
        }
        config.device = self.device.clone();
        config.language = self.language.clone();
        config.text_filters = self.text_filters.clone();
        config.typing_mode = self.typing_mode;
        config.progressive_typing = self.progressive_typing;
        config.auto_correction = Some(self.auto_correction);
        config.auto_enter = self.auto_enter;
        config.cue_volume = self.cue_volume;
        config.save_to_clipboard = self.save_to_clipboard;
        config.vad.duck_enabled = self.duck_enabled;
        config.vad.duck_percent = self.duck_percent;
        config.bash_mode = self.bash_mode;
        config.guided_grammar = self.guided_grammar.clone();
        if let Err(e) = config.save() {
            tracing::warn!("Failed to save config: {}", e);
        }
    }

    /// Open the device picker
    fn open_device_picker(&mut self) {
        if self.env_device {
            self.add_log("Cannot change device: set via EARS_DEVICE env var");
            return;
        }
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
            KeyCode::Char('j') | KeyCode::Down if !self.device_picker_devices.is_empty() => {
                self.device_picker_selected =
                    (self.device_picker_selected + 1) % self.device_picker_devices.len();
            }
            KeyCode::Char('k') | KeyCode::Up if !self.device_picker_devices.is_empty() => {
                self.device_picker_selected = if self.device_picker_selected == 0 {
                    self.device_picker_devices.len() - 1
                } else {
                    self.device_picker_selected - 1
                };
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
            StreamingEvent::SpeechProbable => {
                crate::desktop::AudioFeedback::beep_vad_speech_start().ok();
                self.ducker.on_speech_probable();
            }
            StreamingEvent::SpeechStarted => {
                self.is_speaking = true;
                crate::desktop::AudioFeedback::beep_vad_speech_confirm().ok();
            }
            StreamingEvent::SpeechEnded => {
                self.is_speaking = false;
                crate::desktop::AudioFeedback::beep_vad_end().ok();
                self.ducker.on_speech_ended();
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
                self.total_transcriptions += 1;
                self.successful_transcriptions += 1;
                self.total_words += text.split_whitespace().count();
                // Copy to clipboard if enabled
                if self.save_to_clipboard && !text.is_empty() {
                    crate::desktop::TextInput::copy_to_clipboard(&text);
                }
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
                self.total_transcriptions += 1;
                self.failed_transcriptions += 1;
            }
        }
    }

    /// Handle a tick event
    pub fn handle_tick(&mut self) {
        self.tick_count += 1;

        // Lazy fetch model on first tick, only if not already configured
        if self.tick_count == 1 && self.model == "(connecting...)" {
            self.trigger_model_fetch();
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    // --- Panel navigation ---

    #[test]
    fn test_panel_navigation_tab() {
        let mut app = App::new();
        assert_eq!(app.current_panel, Panel::Configuration);

        app.handle_key(key(KeyCode::Tab)).unwrap();
        assert_eq!(app.current_panel, Panel::Logs);

        app.handle_key(key(KeyCode::Tab)).unwrap();
        assert_eq!(app.current_panel, Panel::LiveTranscription);

        app.handle_key(key(KeyCode::Tab)).unwrap();
        assert_eq!(app.current_panel, Panel::Configuration);
    }

    #[test]
    fn test_panel_navigation_hl() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Char('h'))).unwrap();
        assert_eq!(app.current_panel, Panel::LiveTranscription);

        app.handle_key(key(KeyCode::Char('h'))).unwrap();
        assert_eq!(app.current_panel, Panel::Logs);

        // Wrap backwards
        app.handle_key(key(KeyCode::Char('h'))).unwrap();
        assert_eq!(app.current_panel, Panel::Configuration);
    }

    // --- Help overlay ---

    #[test]
    fn test_help_overlay_toggle() {
        let mut app = App::new();
        assert!(!app.help_overlay_open);

        app.handle_key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help_overlay_open);

        // Other keys should be absorbed
        app.handle_key(key(KeyCode::Char('q'))).unwrap();
        assert!(app.help_overlay_open); // Should still be open, not quit

        // Close with Esc
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.help_overlay_open);
    }

    #[test]
    fn test_help_overlay_close_with_question_mark() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Char('?'))).unwrap();
        assert!(app.help_overlay_open);

        app.handle_key(key(KeyCode::Char('?'))).unwrap();
        assert!(!app.help_overlay_open);
    }

    // --- Command mode ---

    #[test]
    fn test_command_mode_enter_exit() {
        let mut app = App::new();
        assert!(!app.command_mode);

        app.handle_key(key(KeyCode::Char(':'))).unwrap();
        assert!(app.command_mode);

        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.command_mode);
    }

    #[test]
    fn test_command_mode_quit() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Char(':'))).unwrap();
        app.handle_key(key(KeyCode::Char('q'))).unwrap();
        let should_continue = app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(!should_continue);
    }

    #[test]
    fn test_command_mode_unknown_command() {
        let mut app = App::new();
        let initial_logs = app.logs.len();

        app.handle_key(key(KeyCode::Char(':'))).unwrap();
        app.handle_key(key(KeyCode::Char('x'))).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();

        assert!(app.logs.len() > initial_logs);
        assert!(app.logs.last().unwrap().contains("Unknown command"));
    }

    #[test]
    fn test_command_theme_toggle() {
        let mut app = App::new();
        assert_eq!(app.theme_name, ThemeName::Dark);

        // :theme toggles
        app.handle_key(key(KeyCode::Char(':'))).unwrap();
        for c in "theme".chars() {
            app.handle_key(key(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();

        assert_eq!(app.theme_name, ThemeName::Light);
    }

    #[test]
    fn test_command_theme_set_specific() {
        let mut app = App::new();

        // :theme dark
        app.handle_key(key(KeyCode::Char(':'))).unwrap();
        for c in "theme dark".chars() {
            app.handle_key(key(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();

        assert_eq!(app.theme_name, ThemeName::Dark);
    }

    // --- Log search ---

    #[test]
    fn test_search_mode_enter_exit() {
        let mut app = App::new();
        app.current_panel = Panel::Logs;

        app.handle_key(key(KeyCode::Char('/'))).unwrap();
        assert!(app.search_mode);

        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.search_mode);
    }

    #[test]
    fn test_search_finds_matches() {
        let mut app = App::new();
        app.current_panel = Panel::Logs;
        app.logs = vec![
            "Application started".to_string(),
            "Error: connection failed".to_string(),
            "TUI initialized".to_string(),
        ];

        app.handle_key(key(KeyCode::Char('/'))).unwrap();
        for c in "error".chars() {
            app.handle_key(key(KeyCode::Char(c))).unwrap();
        }

        assert_eq!(app.search_matches.len(), 1);
        assert_eq!(app.search_matches[0], 1);
    }

    #[test]
    fn test_search_navigation_n() {
        let mut app = App::new();
        app.current_panel = Panel::Logs;
        app.logs = vec![
            "error one".to_string(),
            "ok".to_string(),
            "error two".to_string(),
        ];

        // Enter search and type query
        app.handle_key(key(KeyCode::Char('/'))).unwrap();
        for c in "error".chars() {
            app.handle_key(key(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();

        assert_eq!(app.search_matches, vec![0, 2]);
        assert_eq!(app.selected_log, 0);

        // n goes to next match
        app.handle_key(key(KeyCode::Char('n'))).unwrap();
        assert_eq!(app.selected_log, 2);

        // n wraps around
        app.handle_key(key(KeyCode::Char('n'))).unwrap();
        assert_eq!(app.selected_log, 0);
    }

    #[test]
    fn test_search_navigation_shift_n() {
        let mut app = App::new();
        app.current_panel = Panel::Logs;
        app.logs = vec![
            "error one".to_string(),
            "ok".to_string(),
            "error two".to_string(),
        ];

        app.handle_key(key(KeyCode::Char('/'))).unwrap();
        for c in "error".chars() {
            app.handle_key(key(KeyCode::Char(c))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();

        // N goes to previous (wraps to last)
        app.handle_key(shift_key(KeyCode::Char('N'))).unwrap();
        assert_eq!(app.selected_log, 2);
    }

    // --- Log filtering ---

    #[test]
    fn test_log_filter_cycle() {
        let mut app = App::new();
        app.current_panel = Panel::Logs;
        assert_eq!(app.log_filter, LogFilter::All);

        app.handle_key(shift_key(KeyCode::Char('F'))).unwrap();
        assert_eq!(app.log_filter, LogFilter::Errors);

        app.handle_key(shift_key(KeyCode::Char('F'))).unwrap();
        assert_eq!(app.log_filter, LogFilter::Warnings);

        app.handle_key(shift_key(KeyCode::Char('F'))).unwrap();
        assert_eq!(app.log_filter, LogFilter::All);
    }

    #[test]
    fn test_log_filter_matches() {
        assert!(LogFilter::All.matches("anything"));
        assert!(LogFilter::Errors.matches("Connection failed"));
        assert!(LogFilter::Errors.matches("Streaming error: timeout"));
        assert!(!LogFilter::Errors.matches("Application started"));
        assert!(LogFilter::Warnings.matches("server offline"));
        assert!(!LogFilter::Warnings.matches("Configuration saved"));
    }

    // --- Scrolling ---

    #[test]
    fn test_scroll_in_logs_panel() {
        let mut app = App::new();
        app.current_panel = Panel::Logs;
        app.logs = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        app.selected_log = 0;

        app.handle_key(key(KeyCode::Char('j'))).unwrap();
        assert_eq!(app.selected_log, 1);

        app.handle_key(key(KeyCode::Char('j'))).unwrap();
        assert_eq!(app.selected_log, 2);

        // Can't scroll past end
        app.handle_key(key(KeyCode::Char('j'))).unwrap();
        assert_eq!(app.selected_log, 2);

        app.handle_key(key(KeyCode::Char('k'))).unwrap();
        assert_eq!(app.selected_log, 1);
    }

    // --- VAD toggle ---

    #[test]
    fn test_vad_toggle() {
        let mut app = App::new();
        assert!(!app.vad_active);

        app.handle_key(key(KeyCode::Char('v'))).unwrap();
        assert!(app.vad_active);

        app.handle_key(key(KeyCode::Char('v'))).unwrap();
        assert!(!app.vad_active);
    }

    // --- Space key ---

    #[test]
    fn test_space_toggles_vad() {
        let mut app = App::new();
        assert!(!app.vad_active);

        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert!(app.vad_active);
    }

    // --- Config panel toggles ---

    #[test]
    fn test_toggle_lowercase_filter() {
        let mut app = App::new();
        app.current_panel = Panel::Configuration;
        let initial = app.text_filters.lowercase;

        app.handle_key(key(KeyCode::Char('f'))).unwrap();
        assert_ne!(app.text_filters.lowercase, initial);
    }

    #[test]
    fn test_toggle_punctuation_filter() {
        let mut app = App::new();
        app.current_panel = Panel::Configuration;
        let initial = app.text_filters.remove_punctuation;

        app.handle_key(key(KeyCode::Char('p'))).unwrap();
        assert_ne!(app.text_filters.remove_punctuation, initial);
    }

    #[test]
    fn test_toggle_progressive_typing_in_configuration() {
        let mut app = App::new();
        app.current_panel = Panel::Configuration;
        let initial = app.progressive_typing;

        app.handle_key(key(KeyCode::Char('t'))).unwrap();
        assert_ne!(app.progressive_typing, initial);
    }

    #[test]
    fn test_toggle_auto_correction_in_configuration() {
        let mut app = App::new();
        app.current_panel = Panel::Configuration;
        let initial = app.auto_correction;

        app.handle_key(key(KeyCode::Char('a'))).unwrap();
        assert_ne!(app.auto_correction, initial);
    }

    // --- Live panel toggles ---

    #[test]
    fn test_toggle_progressive_typing_in_live() {
        let mut app = App::new();
        app.current_panel = Panel::LiveTranscription;
        let initial = app.progressive_typing;

        app.handle_key(key(KeyCode::Char('t'))).unwrap();
        assert_ne!(app.progressive_typing, initial);
    }

    #[test]
    fn test_toggle_auto_correction_in_live() {
        let mut app = App::new();
        app.current_panel = Panel::LiveTranscription;
        let initial = app.auto_correction;

        app.handle_key(key(KeyCode::Char('a'))).unwrap();
        assert_ne!(app.auto_correction, initial);
    }

    // --- Quit ---

    #[test]
    fn test_quit_with_q() {
        let mut app = App::new();
        let result = app.handle_key(key(KeyCode::Char('q'))).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_quit_with_esc() {
        let mut app = App::new();
        let result = app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!result);
    }

    // --- Streaming events ---

    #[test]
    fn test_handle_segment_completed() {
        let mut app = App::new();
        app.handle_streaming_event(StreamingEvent::SegmentCompleted {
            text: "hello world".to_string(),
            duration_ms: 500,
        });

        assert_eq!(app.total_transcriptions, 1);
        assert_eq!(app.successful_transcriptions, 1);
        assert_eq!(app.total_words, 2);
    }

    #[test]
    fn test_handle_streaming_error() {
        let mut app = App::new();
        app.handle_streaming_event(StreamingEvent::Error("timeout".to_string()));

        assert_eq!(app.total_transcriptions, 1);
        assert_eq!(app.failed_transcriptions, 1);
    }

    // --- Env var protection ---

    #[test]
    fn test_env_server_blocks_edit() {
        let mut app = App::new();
        app.env_server = true;
        app.current_panel = Panel::Configuration;

        app.handle_key(key(KeyCode::Char('e'))).unwrap();
        assert!(app.editing_field.is_none());
        assert!(app.logs.last().unwrap().contains("Cannot edit"));
    }

    // --- Add log auto-scroll ---

    #[test]
    fn test_add_log_auto_scrolls() {
        let mut app = App::new();
        // Position at last log
        app.selected_log = app.logs.len() - 1;

        app.add_log("new message");
        assert_eq!(app.selected_log, app.logs.len() - 1);
    }

    #[test]
    fn test_add_log_no_scroll_if_not_at_end() {
        let mut app = App::new();
        app.logs = vec!["a".to_string(), "b".to_string()];
        app.selected_log = 0;

        app.add_log("c");
        assert_eq!(app.selected_log, 0);
    }
}
