use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use url::Url;

/// Configuration for the ears daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Whisper server URL
    pub whisper_server: Url,
    /// Audio input device name
    pub device: String,
    /// Configuration directory
    #[serde(skip)]
    pub config_dir: PathBuf,
    /// State directory (runtime files)
    #[serde(skip)]
    pub state_dir: PathBuf,
}

impl Config {
    /// Create a new Config with default values
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from("com", "heiervang", "ears")
            .context("Failed to determine project directories")?;

        let config_dir = project_dirs.config_dir().to_path_buf();
        let state_dir = std::env::var("XDG_RUNTIME_DIR")
            .map(|p| PathBuf::from(p).join("ears"))
            .unwrap_or_else(|_| PathBuf::from("/tmp").join(format!("ears-{}", std::process::id())));

        Ok(Self {
            whisper_server: Url::parse("http://127.0.0.1:8178")
                .expect("Default server URL is valid"),
            device: "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
                .to_string(),
            config_dir,
            state_dir,
        })
    }

    /// Load configuration from environment variables
    ///
    /// Supports the following environment variables:
    /// - EARS_SERVER: Whisper server URL
    /// - EARS_DEVICE: Audio device name
    pub fn from_env() -> Result<Self> {
        let mut config = Self::new()?;

        if let Ok(server) = std::env::var("EARS_SERVER") {
            config.whisper_server = Url::parse(&server)
                .with_context(|| format!("Invalid EARS_SERVER URL: {}", server))?;
        }

        if let Ok(device) = std::env::var("EARS_DEVICE") {
            config.device = device;
        }

        Ok(config)
    }

    /// Load configuration from file
    ///
    /// Reads from:
    /// - `~/.config/ears/server` for whisper server URL
    /// - `~/.config/ears/device` for audio device name
    pub fn load() -> Result<Self> {
        let mut config = Self::from_env()?;

        // Ensure config directory exists
        fs::create_dir_all(&config.config_dir).context("Failed to create config directory")?;

        // Load server URL if file exists
        let server_file = config.config_dir.join("server");
        if server_file.exists() {
            let server_str = fs::read_to_string(&server_file)
                .context("Failed to read server config file")?
                .trim()
                .to_string();

            config.whisper_server = Url::parse(&server_str)
                .with_context(|| format!("Invalid server URL in config file: {}", server_str))?;
        }

        // Load device if file exists
        let device_file = config.config_dir.join("device");
        if device_file.exists() {
            config.device = fs::read_to_string(&device_file)
                .context("Failed to read device config file")?
                .trim()
                .to_string();
        }

        // Ensure state directory exists
        fs::create_dir_all(&config.state_dir).context("Failed to create state directory")?;

        Ok(config)
    }

    /// Save configuration to file
    ///
    /// Writes to:
    /// - `~/.config/ears/server` for whisper server URL
    /// - `~/.config/ears/device` for audio device name
    pub fn save(&self) -> Result<()> {
        // Ensure config directory exists
        fs::create_dir_all(&self.config_dir).context("Failed to create config directory")?;

        // Save server URL
        let server_file = self.config_dir.join("server");
        fs::write(&server_file, self.whisper_server.as_str())
            .context("Failed to write server config file")?;

        // Save device
        let device_file = self.config_dir.join("device");
        fs::write(&device_file, &self.device).context("Failed to write device config file")?;

        Ok(())
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate server URL scheme
        if !matches!(self.whisper_server.scheme(), "http" | "https") {
            anyhow::bail!(
                "Invalid server URL scheme: {} (must be http or https)",
                self.whisper_server.scheme()
            );
        }

        // Validate server URL has a host
        if self.whisper_server.host().is_none() {
            anyhow::bail!("Server URL must have a host");
        }

        // Validate device is not empty
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
        assert!(!config.device.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let (mut config, _temp_dir) = setup_test_config();

        config.whisper_server = Url::parse("http://localhost:9000").unwrap();
        config.device = "test-device".to_string();

        config.save().unwrap();

        // Load config and verify
        let loaded = {
            let mut new_config = Config::new().unwrap();
            new_config.config_dir = config.config_dir.clone();
            new_config.state_dir = config.state_dir.clone();

            // Load from files
            let server_file = new_config.config_dir.join("server");
            let server_str = fs::read_to_string(&server_file).unwrap().trim().to_string();
            new_config.whisper_server = Url::parse(&server_str).unwrap();

            let device_file = new_config.config_dir.join("device");
            new_config.device = fs::read_to_string(&device_file).unwrap().trim().to_string();

            new_config
        };

        assert_eq!(loaded.whisper_server.as_str(), "http://localhost:9000/");
        assert_eq!(loaded.device, "test-device");
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
    #[serial_test::serial]
    fn test_from_env() {
        std::env::set_var("EARS_SERVER", "http://test-server:8080");
        std::env::set_var("EARS_DEVICE", "test-mic");

        let config = Config::from_env().unwrap();
        assert_eq!(config.whisper_server.as_str(), "http://test-server:8080/");
        assert_eq!(config.device, "test-mic");

        std::env::remove_var("EARS_SERVER");
        std::env::remove_var("EARS_DEVICE");
    }

    #[test]
    #[serial_test::serial]
    fn test_from_env_invalid_url() {
        std::env::set_var("EARS_SERVER", "not-a-valid-url");

        let result = Config::from_env();
        assert!(result.is_err());

        std::env::remove_var("EARS_SERVER");
    }
}
