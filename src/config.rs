use crate::desktop::TypingMode;
use crate::text_filters::TextFilters;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

fn default_server() -> Url {
    Url::parse("http://127.0.0.1:8178").expect("Default server URL is valid")
}

fn default_device() -> String {
    "default".to_string()
}

fn default_speech_threshold() -> f32 {
    0.5
}

fn default_min_speech_duration_ms() -> u64 {
    300
}

fn default_max_silence_duration_ms() -> u64 {
    700
}

fn default_pre_speech_buffer_ms() -> u64 {
    500
}

fn default_auto_enter() -> bool {
    true
}

fn default_progressive_typing() -> bool {
    false
}

fn default_cue_volume() -> u8 {
    100
}

fn default_save_to_clipboard() -> bool {
    false
}

/// Language-specific ASR server override
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageServer {
    /// ASR server URL for this language
    pub server: Url,
    /// Model name to send with requests (optional)
    pub model: Option<String>,
}

/// VAD (Voice Activity Detection) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadSettings {
    /// Speech probability threshold (0.0-1.0, default: 0.5)
    /// Higher = fewer false positives, lower = more sensitive
    #[serde(default = "default_speech_threshold")]
    pub speech_threshold: f32,
    /// Minimum speech duration in ms before segment starts (default: 300)
    #[serde(default = "default_min_speech_duration_ms")]
    pub min_speech_duration_ms: u64,
    /// Maximum silence duration in ms before segment ends (default: 700)
    #[serde(default = "default_max_silence_duration_ms")]
    pub max_silence_duration_ms: u64,
    /// Pre-speech replay buffer in ms (default: 500)
    /// Keeps recent audio so utterance onsets are not clipped
    #[serde(default = "default_pre_speech_buffer_ms")]
    pub pre_speech_buffer_ms: u64,
}

impl Default for VadSettings {
    fn default() -> Self {
        Self {
            speech_threshold: default_speech_threshold(),
            min_speech_duration_ms: default_min_speech_duration_ms(),
            max_silence_duration_ms: default_max_silence_duration_ms(),
            pre_speech_buffer_ms: default_pre_speech_buffer_ms(),
        }
    }
}

/// Configuration for the ears daemon
///
/// Loaded from `~/.config/ears/config.toml` (or `config.{profile}.toml`).
/// Environment variables override file values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Whisper server URL
    #[serde(rename = "server", default = "default_server")]
    pub whisper_server: Url,
    /// Audio input device name
    #[serde(default = "default_device")]
    pub device: String,
    /// Language code for transcription (None = auto-detect)
    pub language: Option<String>,
    /// API key for authenticated ASR services (None = no auth)
    pub api_key: Option<String>,
    /// Model name for transcription (None = server default)
    pub model: Option<String>,
    /// Prompt for context biasing (entity names, acronyms, domain terms)
    /// Passed as the `prompt` field in the OpenAI transcription API.
    /// Example: "Soumith Chintala, Safetensors, vLLM, PyTorch"
    pub prompt: Option<String>,
    /// Text filters for transcription output
    #[serde(default)]
    pub text_filters: TextFilters,
    /// Text input method (auto/wtype/paste)
    #[serde(default)]
    pub typing_mode: TypingMode,
    /// Send Enter key after each transcription (default: true)
    #[serde(default = "default_auto_enter")]
    pub auto_enter: bool,
    /// Enable progressive typing in streaming mode (default: false)
    #[serde(default = "default_progressive_typing")]
    pub progressive_typing: bool,
    /// Save transcribed text to clipboard after each segment (default: false)
    #[serde(default = "default_save_to_clipboard")]
    pub save_to_clipboard: bool,
    /// Enable auto-correction for progressive typing (None = legacy behavior)
    #[serde(default)]
    pub auto_correction: Option<bool>,
    /// Enable bash mode: constrain ASR output to a shell grammar via the
    /// chat-completions endpoint (constrained decoding). When on, spoken
    /// commands are biased toward valid bash syntax. Default: false.
    #[serde(default)]
    pub bash_mode: bool,
    /// Custom guided grammar (GBNF) override. When None and `bash_mode` is on,
    /// the built-in bash grammar ([`Config::BASH_GRAMMAR`]) is used. Default: None.
    #[serde(default)]
    pub guided_grammar: Option<String>,
    /// Audio cue volume (0-100, default: 100)
    #[serde(default = "default_cue_volume")]
    pub cue_volume: u8,
    /// Language-specific ASR server overrides.
    /// Maps language codes (e.g. "no", "de") to server + optional model.
    /// When the detected keyboard language matches a key, that server is used
    /// instead of the default `server`.
    #[serde(default)]
    pub language_servers: HashMap<String, LanguageServer>,
    /// VAD settings
    #[serde(default)]
    pub vad: VadSettings,
    /// Configuration directory (computed, not stored)
    #[serde(skip)]
    pub config_dir: PathBuf,
    /// Active profile name used to load this config (None = default config.toml)
    #[serde(skip)]
    pub active_profile: Option<String>,
    /// State directory (computed, not stored)
    #[serde(skip)]
    pub state_dir: PathBuf,
}

impl Config {
    /// Compute config and state directory paths
    fn computed_dirs() -> Result<(PathBuf, PathBuf)> {
        let project_dirs = ProjectDirs::from("com", "heiervang", "ears")
            .context("Failed to determine project directories")?;
        let config_dir = project_dirs.config_dir().to_path_buf();
        let state_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(|p| PathBuf::from(p).join("ears"))
            .unwrap_or_else(|_| project_dirs.cache_dir().join("run"));
        Ok((config_dir, state_dir))
    }

    /// Get config file path for a given profile
    fn config_file_path(config_dir: &Path, profile: Option<&str>) -> PathBuf {
        match profile {
            Some(name) => config_dir.join(format!("config.{}.toml", name)),
            None => config_dir.join("config.toml"),
        }
    }

    /// Create a new Config with default values
    pub fn new() -> Result<Self> {
        let (config_dir, state_dir) = Self::computed_dirs()?;
        Ok(Self {
            whisper_server: default_server(),
            device: default_device(),
            language: None,
            api_key: None,
            model: None,
            prompt: None,
            text_filters: TextFilters::new(),
            typing_mode: TypingMode::default(),
            auto_enter: true,
            progressive_typing: false,
            save_to_clipboard: false,
            auto_correction: None,
            bash_mode: false,
            guided_grammar: None,
            cue_volume: default_cue_volume(),
            language_servers: HashMap::new(),
            vad: VadSettings::default(),
            config_dir,
            active_profile: None,
            state_dir,
        })
    }

    /// Load configuration with an optional profile name
    ///
    /// Priority: env vars > config file > defaults
    ///
    /// Profile resolution: `profile` arg > `EARS_PROFILE` env var > default
    pub fn load_profile(profile: Option<&str>) -> Result<Self> {
        let (config_dir, state_dir) = Self::computed_dirs()?;
        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        // Resolve profile: CLI arg > env var > persistent file
        let env_profile = std::env::var("EARS_PROFILE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let file_profile = fs::read_to_string(config_dir.join("profile"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let profile_name = profile
            .map(|s| s.to_string())
            .or(env_profile)
            .or(file_profile);

        let config_file = Self::config_file_path(&config_dir, profile_name.as_deref());

        let mut config = if config_file.exists() {
            let content = fs::read_to_string(&config_file)
                .with_context(|| format!("Failed to read {}", config_file.display()))?;
            toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", config_file.display()))?
        } else if profile_name.is_some() {
            anyhow::bail!("Profile config not found: {}", config_file.display());
        } else {
            // No config.toml — try migrating old files or use defaults
            Self::migrate_old_files(&config_dir)?
        };

        config.config_dir = config_dir;
        config.active_profile = profile_name;
        config.state_dir = state_dir;

        // Apply env var overrides (highest priority)
        config.apply_env_overrides()?;

        // Warn about a common cloud-config mistake: appending /v1 to the server
        // URL. ears adds /v1/audio/transcriptions itself.
        if config.server_has_redundant_v1() {
            tracing::warn!(
                "Server URL '{}' ends in /v1; ears appends /v1/audio/transcriptions itself, \
                 so requests will hit a doubled /v1/v1 path. Drop the trailing /v1.",
                config.whisper_server
            );
        }

        // Ensure state directory exists
        fs::create_dir_all(&config.state_dir).context("Failed to create state directory")?;

        Ok(config)
    }

    /// Load configuration (default profile)
    pub fn load() -> Result<Self> {
        Self::load_profile(None)
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&mut self) -> Result<()> {
        if let Ok(server) = std::env::var("EARS_SERVER") {
            self.whisper_server = Url::parse(&server)
                .with_context(|| format!("Invalid EARS_SERVER URL: {}", server))?;
        }
        if let Ok(device) = std::env::var("EARS_DEVICE") {
            let device = device.trim().to_string();
            if !device.is_empty() {
                self.device = device;
            }
        }
        if let Ok(language) = std::env::var("EARS_LANGUAGE") {
            let language = language.trim();
            self.language = if language.is_empty() {
                None
            } else {
                Some(language.to_string())
            };
        }
        if let Ok(api_key) = std::env::var("EARS_API_KEY") {
            let api_key = api_key.trim().to_string();
            self.api_key = if api_key.is_empty() {
                None
            } else {
                Some(api_key)
            };
        }
        if let Ok(model) = std::env::var("EARS_MODEL") {
            let model = model.trim().to_string();
            self.model = if model.is_empty() { None } else { Some(model) };
        }
        if let Ok(prompt) = std::env::var("EARS_PROMPT") {
            let prompt = prompt.trim().to_string();
            self.prompt = if prompt.is_empty() {
                None
            } else {
                Some(prompt)
            };
        }
        Ok(())
    }

    /// Migrate from old multi-file config to config.toml
    fn migrate_old_files(config_dir: &Path) -> Result<Self> {
        let mut config = Self {
            whisper_server: default_server(),
            device: default_device(),
            language: None,
            api_key: None,
            model: None,
            prompt: None,
            text_filters: TextFilters::new(),
            typing_mode: TypingMode::default(),
            auto_enter: true,
            progressive_typing: false,
            save_to_clipboard: false,
            auto_correction: None,
            bash_mode: false,
            guided_grammar: None,
            cue_volume: default_cue_volume(),
            language_servers: HashMap::new(),
            vad: VadSettings::default(),
            config_dir: PathBuf::new(),
            active_profile: None,
            state_dir: PathBuf::new(),
        };

        let mut migrated_any = false;

        if let Ok(s) = fs::read_to_string(config_dir.join("server")) {
            if let Ok(url) = Url::parse(s.trim()) {
                config.whisper_server = url;
                migrated_any = true;
            }
        }
        if let Ok(s) = fs::read_to_string(config_dir.join("device")) {
            let d = s.trim().to_string();
            if !d.is_empty() {
                config.device = d;
                migrated_any = true;
            }
        }
        if let Ok(s) = fs::read_to_string(config_dir.join("language")) {
            let l = s.trim().to_string();
            if !l.is_empty() {
                config.language = Some(l);
                migrated_any = true;
            }
        }
        if let Ok(s) = fs::read_to_string(config_dir.join("api_key")) {
            let k = s.trim().to_string();
            if !k.is_empty() {
                config.api_key = Some(k);
                migrated_any = true;
            }
        }
        if let Ok(s) = fs::read_to_string(config_dir.join("model")) {
            let m = s.trim().to_string();
            if !m.is_empty() {
                config.model = Some(m);
                migrated_any = true;
            }
        }
        if let Ok(s) = fs::read_to_string(config_dir.join("text_filters.json")) {
            if let Ok(filters) = serde_json::from_str(&s) {
                config.text_filters = filters;
                migrated_any = true;
            }
        }

        // Write migrated config as config.toml
        if migrated_any {
            let toml_path = config_dir.join("config.toml");
            if let Ok(toml_str) = toml::to_string_pretty(&config) {
                fs::write(&toml_path, &toml_str).ok();
                tracing::info!("Migrated old config files to {}", toml_path.display());
                eprintln!("Migrated config to {}", toml_path.display());
            }
        }

        Ok(config)
    }

    /// List available profile names by scanning config.*.toml files
    pub fn list_profiles() -> Result<Vec<String>> {
        let (config_dir, _) = Self::computed_dirs()?;
        let mut profiles = Vec::new();
        if let Ok(entries) = fs::read_dir(&config_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(profile) = name
                    .strip_prefix("config.")
                    .and_then(|s| s.strip_suffix(".toml"))
                {
                    profiles.push(profile.to_string());
                }
            }
        }
        profiles.sort();
        Ok(profiles)
    }

    /// Get the currently persisted default profile name
    pub fn get_default_profile() -> Result<Option<String>> {
        let (config_dir, _) = Self::computed_dirs()?;
        let profile_file = config_dir.join("profile");
        Ok(fs::read_to_string(profile_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    }

    /// Set the default profile (persists to ~/.config/ears/profile)
    pub fn set_default_profile(name: &str) -> Result<()> {
        let (config_dir, _) = Self::computed_dirs()?;
        fs::create_dir_all(&config_dir)?;

        if name == "default" || name.is_empty() {
            // Clear the profile
            let profile_file = config_dir.join("profile");
            if profile_file.exists() {
                fs::remove_file(&profile_file)?;
            }
            return Ok(());
        }

        // Validate that the profile config exists
        let config_file = config_dir.join(format!("config.{}.toml", name));
        if !config_file.exists() {
            anyhow::bail!(
                "Profile '{}' not found (expected {})",
                name,
                config_file.display()
            );
        }

        fs::write(config_dir.join("profile"), name)?;
        Ok(())
    }

    /// Save configuration to TOML file
    ///
    /// The file is written with `0600` permissions because it may contain a
    /// plaintext `api_key` for cloud ASR services.
    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(&self.config_dir).context("Failed to create config directory")?;
        let config_file = Self::config_file_path(&self.config_dir, self.active_profile.as_deref());
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_file, toml_str)
            .with_context(|| format!("Failed to write {}", config_file.display()))?;
        Self::restrict_permissions(&config_file);
        Ok(())
    }

    /// Restrict a config file to owner read/write only (`0600`).
    ///
    /// Config files can hold a plaintext `api_key`, so they should not be
    /// world- or group-readable. Best-effort: failures are logged, not fatal.
    #[cfg(unix)]
    fn restrict_permissions(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            tracing::warn!(
                "Failed to restrict permissions on {}: {}",
                path.display(),
                e
            );
        }
    }

    #[cfg(not(unix))]
    fn restrict_permissions(_path: &Path) {}

    /// Return the resolved config file path this instance reads/writes.
    pub fn config_file(&self) -> PathBuf {
        Self::config_file_path(&self.config_dir, self.active_profile.as_deref())
    }

    /// Built-in bash dictation grammar (GBNF), embedded at compile time.
    ///
    /// Constrains ASR output to valid bash syntax when [`Config::bash_mode`] is
    /// on and no custom [`Config::guided_grammar`] is set. See
    /// `grammars/bash.gbnf` for the grammar and the rationale behind the
    /// `scaffold` rule.
    pub const BASH_GRAMMAR: &'static str = include_str!("../grammars/bash.gbnf");

    /// Resolve the active guided grammar for constrained decoding.
    ///
    /// Returns `Some(grammar)` only when `bash_mode` is enabled (using the
    /// custom `guided_grammar` if set, otherwise the built-in bash grammar).
    /// Returns `None` when bash mode is off, which keeps ears on the plain
    /// transcription endpoint. Constrained decoding is therefore gated on
    /// `bash_mode`.
    pub fn active_grammar(&self) -> Option<String> {
        if !self.bash_mode {
            return None;
        }
        Some(
            self.guided_grammar
                .clone()
                .unwrap_or_else(|| Self::BASH_GRAMMAR.to_string()),
        )
    }

    /// Resolve effective auto-correction setting with backward compatibility.
    pub fn effective_auto_correction(&self) -> bool {
        self.auto_correction.unwrap_or(self.progressive_typing)
    }

    /// Save text filter settings (saves the full active config file)
    pub fn save_text_filters(&self) -> Result<()> {
        self.save()
    }

    /// Resolve the ASR server URL and model for a given language.
    ///
    /// If `language_servers` contains an entry for the language code, that
    /// server (and optional model) is returned. Otherwise falls back to the
    /// default `whisper_server` and `model`.
    pub fn resolve_server(&self, language: Option<&str>) -> (Url, Option<String>) {
        if let Some(lang) = language {
            if let Some(ls) = self.language_servers.get(lang) {
                tracing::debug!(
                    "Using language-specific server for '{}': {}",
                    lang,
                    ls.server
                );
                return (
                    ls.server.clone(),
                    ls.model.clone().or_else(|| self.model.clone()),
                );
            }
        }
        (self.whisper_server.clone(), self.model.clone())
    }

    /// Returns true if the configured server URL path ends in a `/v1` segment.
    ///
    /// ears appends `/v1/audio/transcriptions` to the server URL itself, so a
    /// server that already ends in `/v1` (as some cloud provider docs present
    /// their base URL) produces a doubled `/v1/v1/audio/transcriptions` path
    /// that 404s. Used to surface a warning rather than silently rewriting the
    /// URL (which would break servers that legitimately live under `/v1`).
    pub fn server_has_redundant_v1(&self) -> bool {
        self.whisper_server
            .path()
            .trim_end_matches('/')
            .ends_with("/v1")
    }

    /// Validate the configuration
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.whisper_server.scheme(), "http" | "https") {
            anyhow::bail!(
                "Invalid server URL scheme: {} (must be http or https)",
                self.whisper_server.scheme()
            );
        }
        if self.whisper_server.host().is_none() {
            anyhow::bail!("Server URL must have a host");
        }
        if self.device.is_empty() {
            anyhow::bail!("Device name cannot be empty");
        }
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_config() -> (Config, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let mut config = Config::new().unwrap();
        config.config_dir = temp_dir.path().to_path_buf();
        config.state_dir = temp_dir.path().join("state");
        (config, temp_dir)
    }

    #[test]
    fn test_new_config() {
        let config = Config::new().unwrap();
        assert_eq!(config.whisper_server.as_str(), "http://127.0.0.1:8178/");
    }

    #[test]
    fn test_save_and_load_toml() {
        let (mut config, _temp_dir) = setup_test_config();

        config.whisper_server = Url::parse("http://localhost:9000").unwrap();
        config.device = "test-device".to_string();
        config.model = Some("whisper-large-v3-turbo".to_string());

        config.save().unwrap();

        // Verify TOML file exists and can be parsed
        let toml_path = config.config_dir.join("config.toml");
        assert!(toml_path.exists());
        let content = fs::read_to_string(&toml_path).unwrap();
        let loaded: Config = toml::from_str(&content).unwrap();

        assert_eq!(loaded.whisper_server.as_str(), "http://localhost:9000/");
        assert_eq!(loaded.device, "test-device");
        assert_eq!(loaded.model, Some("whisper-large-v3-turbo".to_string()));
    }

    #[test]
    fn test_save_uses_active_profile_file() {
        let (mut config, _temp_dir) = setup_test_config();
        config.active_profile = Some("work".to_string());
        config.auto_enter = false;
        config.progressive_typing = true;
        config.auto_correction = Some(false);

        config.save().unwrap();

        let profile_path = config.config_dir.join("config.work.toml");
        let default_path = config.config_dir.join("config.toml");

        assert!(profile_path.exists());
        assert!(!default_path.exists());

        let content = fs::read_to_string(profile_path).unwrap();
        let loaded: Config = toml::from_str(&content).unwrap();
        assert!(!loaded.auto_enter);
        assert!(loaded.progressive_typing);
        assert_eq!(loaded.auto_correction, Some(false));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = Config::new().unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_scheme() {
        let mut config = Config::new().unwrap();
        config.whisper_server = Url::parse("ftp://localhost:8178").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_device() {
        let mut config = Config::new().unwrap();
        config.device = String::new();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_migration_from_old_files() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();

        // Write old-style config files
        fs::write(config_dir.join("server"), "http://old-server:8080").unwrap();
        fs::write(config_dir.join("device"), "old-device").unwrap();
        fs::write(config_dir.join("language"), "no").unwrap();

        let config = Config::migrate_old_files(&config_dir).unwrap();

        assert_eq!(config.whisper_server.as_str(), "http://old-server:8080/");
        assert_eq!(config.device, "old-device");
        assert_eq!(config.language, Some("no".to_string()));

        // Verify config.toml was created
        assert!(config_dir.join("config.toml").exists());
    }

    #[test]
    fn test_toml_partial_config() {
        // Only some fields specified — others should use defaults
        let toml_str = r#"
server = "http://my-server:9000"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.whisper_server.as_str(), "http://my-server:9000/");
        assert_eq!(config.device, "default");
        assert!(config.language.is_none());
        assert!(config.model.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn test_env_overrides() {
        std::env::set_var("EARS_SERVER", "http://env-server:8080");
        std::env::set_var("EARS_DEVICE", "env-mic");
        std::env::set_var("EARS_MODEL", "env-model");

        let mut config = Config::new().unwrap();
        config.apply_env_overrides().unwrap();

        assert_eq!(config.whisper_server.as_str(), "http://env-server:8080/");
        assert_eq!(config.device, "env-mic");
        assert_eq!(config.model, Some("env-model".to_string()));

        std::env::remove_var("EARS_SERVER");
        std::env::remove_var("EARS_DEVICE");
        std::env::remove_var("EARS_MODEL");
    }

    #[test]
    #[serial_test::serial]
    fn test_env_invalid_url() {
        std::env::set_var("EARS_SERVER", "not-a-valid-url");

        let mut config = Config::new().unwrap();
        let result = config.apply_env_overrides();
        assert!(result.is_err());

        std::env::remove_var("EARS_SERVER");
    }

    #[test]
    fn test_toml_invalid_config() {
        let invalid_toml = r#"
server = [[[not valid toml
"#;
        let result: Result<Config, _> = toml::from_str(invalid_toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_toml_unknown_fields_ignored() {
        let toml_str = r#"
server = "http://localhost:8080"
device = "my-mic"
some_unknown_field = "should be ignored"
another_unknown = 42
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.whisper_server.as_str(), "http://localhost:8080/");
        assert_eq!(config.device, "my-mic");
    }

    #[test]
    fn test_vad_settings_defaults() {
        let vad = VadSettings::default();
        assert_eq!(vad.speech_threshold, 0.5);
        assert_eq!(vad.min_speech_duration_ms, 300);
        assert_eq!(vad.max_silence_duration_ms, 700);
        assert_eq!(vad.pre_speech_buffer_ms, 500);
    }

    #[test]
    fn test_bash_grammar_embedded() {
        // The built-in grammar must be present and contain its load-bearing rules.
        assert!(!Config::BASH_GRAMMAR.trim().is_empty());
        assert!(Config::BASH_GRAMMAR.contains("root"));
        assert!(Config::BASH_GRAMMAR.contains("::="));
        // The scaffold rule is what keeps audio conditioning alive — guard it.
        assert!(Config::BASH_GRAMMAR.contains("<asr_text>"));
    }

    #[test]
    fn test_active_grammar_gated_on_bash_mode() {
        let mut config = Config::new().unwrap();

        // Off by default → no constrained decoding.
        assert!(config.active_grammar().is_none());

        // A custom grammar set but bash_mode off → still none.
        config.guided_grammar = Some("root ::= \"ls\"".to_string());
        assert!(config.active_grammar().is_none());

        // bash_mode on with a custom grammar → the custom grammar.
        config.bash_mode = true;
        assert_eq!(config.active_grammar().as_deref(), Some("root ::= \"ls\""));

        // bash_mode on without a custom grammar → the built-in bash grammar.
        config.guided_grammar = None;
        assert_eq!(
            config.active_grammar().as_deref(),
            Some(Config::BASH_GRAMMAR)
        );
    }

    #[test]
    fn test_bash_mode_roundtrips_toml() {
        let (mut config, _temp_dir) = setup_test_config();
        config.bash_mode = true;
        config.save().unwrap();

        let content = fs::read_to_string(config.config_dir.join("config.toml")).unwrap();
        let loaded: Config = toml::from_str(&content).unwrap();
        assert!(loaded.bash_mode);
    }

    #[test]
    fn test_effective_auto_correction() {
        let mut config = Config::new().unwrap();

        // When auto_correction is None, falls back to progressive_typing value
        config.auto_correction = None;
        config.progressive_typing = false;
        assert!(!config.effective_auto_correction());

        config.progressive_typing = true;
        assert!(config.effective_auto_correction());

        // When auto_correction is Some, uses that value regardless of progressive_typing
        config.auto_correction = Some(true);
        config.progressive_typing = false;
        assert!(config.effective_auto_correction());

        config.auto_correction = Some(false);
        config.progressive_typing = true;
        assert!(!config.effective_auto_correction());
    }

    #[test]
    fn test_config_file_path_with_profile() {
        let dir = PathBuf::from("/tmp/test-ears-config");
        let path = Config::config_file_path(&dir, Some("work"));
        assert_eq!(path, dir.join("config.work.toml"));
    }

    #[test]
    fn test_config_file_path_default() {
        let dir = PathBuf::from("/tmp/test-ears-config");
        let path = Config::config_file_path(&dir, None);
        assert_eq!(path, dir.join("config.toml"));
    }

    #[test]
    fn test_resolve_server_with_language_match() {
        let mut config = Config::new().unwrap();
        let no_url = Url::parse("http://192.168.8.170:30190/").unwrap();
        config.language_servers.insert(
            "no".to_string(),
            LanguageServer {
                server: no_url.clone(),
                model: Some("nb-asr-model".to_string()),
            },
        );

        let (server, model) = config.resolve_server(Some("no"));
        assert_eq!(server, no_url);
        assert_eq!(model, Some("nb-asr-model".to_string()));
    }

    #[test]
    fn test_resolve_server_fallback() {
        let config = Config::new().unwrap();

        // Unknown language falls back to default server
        let (server, model) = config.resolve_server(Some("de"));
        assert_eq!(server, config.whisper_server);
        assert_eq!(model, config.model);

        // No language falls back to default server
        let (server, model) = config.resolve_server(None);
        assert_eq!(server, config.whisper_server);
        assert_eq!(model, config.model);
    }

    #[test]
    #[cfg(unix)]
    fn test_save_restricts_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (mut config, _temp_dir) = setup_test_config();
        config.api_key = Some("gsk_secret".to_string());
        config.save().unwrap();

        let mode = fs::metadata(config.config_file())
            .unwrap()
            .permissions()
            .mode();
        // Only the owner read/write bits should be set.
        assert_eq!(
            mode & 0o777,
            0o600,
            "config should be 0600, got {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn test_server_has_redundant_v1() {
        let mut config = Config::new().unwrap();

        config.whisper_server = Url::parse("https://api.groq.com/openai/v1").unwrap();
        assert!(config.server_has_redundant_v1());

        config.whisper_server = Url::parse("https://api.groq.com/openai/v1/").unwrap();
        assert!(config.server_has_redundant_v1());

        // Correct Groq base — ears appends /v1/audio/transcriptions.
        config.whisper_server = Url::parse("https://api.groq.com/openai").unwrap();
        assert!(!config.server_has_redundant_v1());

        // Local server with no path.
        config.whisper_server = Url::parse("http://127.0.0.1:8178").unwrap();
        assert!(!config.server_has_redundant_v1());
    }

    #[test]
    fn test_resolve_server_inherits_default_model() {
        let mut config = Config::new().unwrap();
        config.model = Some("default-model".to_string());
        config.language_servers.insert(
            "no".to_string(),
            LanguageServer {
                server: Url::parse("http://localhost:9999/").unwrap(),
                model: None, // No model override
            },
        );

        let (_server, model) = config.resolve_server(Some("no"));
        // Should inherit the default model when language server has no model
        assert_eq!(model, Some("default-model".to_string()));
    }
}
