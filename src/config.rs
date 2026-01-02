//! Configuration management for ears
//!
//! Handles loading and saving configuration from files and environment variables.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Whisper server URL
    pub server: String,
    /// Audio device name
    pub device: String,
    /// Model name for display
    pub model: Option<String>,
    /// Recording timeout in seconds
    pub timeout: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: "http://127.0.0.1:8178".to_string(),
            device: "alsa_input.usb-HP__Inc_HyperX_Cloud_II_Wireless_0-00.mono-fallback"
                .to_string(),
            model: None,
            timeout: 120,
        }
    }
}

impl Config {
    /// Get the config directory path
    pub fn config_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".config").join("ears"))
    }

    /// Load server URL from config file
    pub fn load_server() -> Result<String> {
        let config_dir = Self::config_dir()?;
        let server_file = config_dir.join("server");

        if server_file.exists() {
            let content = std::fs::read_to_string(&server_file)
                .context("Failed to read server config file")?;
            Ok(content.trim().to_string())
        } else {
            Ok(Config::default().server)
        }
    }

    /// Save server URL to config file
    pub fn save_server(server: &str) -> Result<()> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let server_file = config_dir.join("server");
        std::fs::write(&server_file, server).context("Failed to write server config file")?;

        Ok(())
    }

    /// Load device name from config file
    pub fn load_device() -> Result<String> {
        let config_dir = Self::config_dir()?;
        let device_file = config_dir.join("device");

        if device_file.exists() {
            let content = std::fs::read_to_string(&device_file)
                .context("Failed to read device config file")?;
            Ok(content.trim().to_string())
        } else {
            Ok(Config::default().device)
        }
    }

    /// Save device name to config file
    pub fn save_device(device: &str) -> Result<()> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let device_file = config_dir.join("device");
        std::fs::write(&device_file, device).context("Failed to write device config file")?;

        Ok(())
    }

    /// Load full configuration
    pub fn load() -> Result<Self> {
        let server = Self::load_server()?;
        let device = Self::load_device()?;

        Ok(Self {
            server,
            device,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server, "http://127.0.0.1:8178");
        assert_eq!(config.timeout, 120);
    }

    #[test]
    fn test_config_round_trip() {
        // Set up temporary HOME
        let temp_dir = tempfile::tempdir().unwrap();
        env::set_var("HOME", temp_dir.path());

        // Save and load server
        Config::save_server("http://test:9000").unwrap();
        let loaded_server = Config::load_server().unwrap();
        assert_eq!(loaded_server, "http://test:9000");

        // Save and load device
        Config::save_device("test-device").unwrap();
        let loaded_device = Config::load_device().unwrap();
        assert_eq!(loaded_device, "test-device");
    }
}
