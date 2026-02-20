use crate::text_filters::TextFilters;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

fn default_server() -> Url {
    Url::parse("http://127.0.0.1:8178").expect("Default server URL is valid")
}

fn default_device() -> String {
    "default".to_string()
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
    /// Text filters for transcription output
    #[serde(default)]
    pub text_filters: TextFilters,
    /// Configuration directory (computed, not stored)
    #[serde(skip)]
    pub config_dir: PathBuf,
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
            .unwrap_or_else(|_| PathBuf::from("/tmp").join(format!("ears-{}", std::process::id())));
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
            text_filters: TextFilters::new(),
            config_dir,
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
        config.state_dir = state_dir;

        // Apply env var overrides (highest priority)
        config.apply_env_overrides()?;

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
            text_filters: TextFilters::new(),
            config_dir: PathBuf::new(),
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
    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(&self.config_dir).context("Failed to create config directory")?;
        let config_file = self.config_dir.join("config.toml");
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_file, toml_str).context("Failed to write config.toml")?;
        Ok(())
    }

    /// Save text filter settings (saves the full config.toml)
    pub fn save_text_filters(&self) -> Result<()> {
        self.save()
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

impl Default for Config {
    fn default() -> Self {
        Self::new().expect("Failed to create default config")
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
}
