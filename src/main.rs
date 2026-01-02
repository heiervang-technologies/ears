mod audio;
mod cli;
mod config;
mod recording;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands};
use config::Config;
use url::Url;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle TUI flag first
    if cli.tui {
        return ears::tui::run();
    }

    match cli.command {
        Some(Commands::Select) => {
            select_device()?;
        }
        Some(Commands::List) => {
            list_devices()?;
        }
        Some(Commands::Current) => {
            show_current()?;
        }
        Some(Commands::Server { url }) => {
            if let Some(url_str) = url {
                set_server(&url_str)?;
            } else {
                show_server()?;
            }
        }
        None => {
            // No command provided - this is the main toggle behavior
            // This will be implemented in later iterations
            eprintln!("Toggle recording/transcription - not yet implemented");
            eprintln!("This functionality will be added in future iterations");
        }
    }

    Ok(())
}

fn select_device() -> Result<()> {
    // List available devices
    let devices = audio::list_devices().context("Failed to list audio devices")?;

    if devices.is_empty() {
        eprintln!("No audio input devices found");
        anyhow::bail!("No audio input devices available");
    }

    // Use fzf for interactive selection
    let selected =
        audio::select_device_interactive(&devices).context("Failed to run device selection")?;

    let device_name = match selected {
        Some(name) => name,
        None => {
            eprintln!("No device selected");
            return Ok(());
        }
    };

    // Find the device to get its description
    let device = devices
        .iter()
        .find(|d| d.name == device_name)
        .context("Selected device not found in list")?;

    // Save to config
    let mut config = Config::load().context("Failed to load configuration")?;
    config.device = device_name.clone();
    config.save().context("Failed to save configuration")?;

    println!("Selected: {}", device.description);
    println!("Device ID: {}", device_name);
    println!("Saved to: {}", config.config_dir.join("device").display());

    Ok(())
}

fn list_devices() -> Result<()> {
    let devices = audio::list_devices().context("Failed to list audio devices")?;

    if devices.is_empty() {
        println!("No audio input devices found");
        return Ok(());
    }

    // Format and print device list
    let formatted = audio::format_device_list(&devices);

    // Use column command to align output nicely
    use std::io::Write;
    use std::process::{Command, Stdio};

    let child = Command::new("column")
        .arg("-t")
        .arg("-s")
        .arg("\t")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(formatted.as_bytes());
            }
            let _ = child.wait();
        }
        Err(_) => {
            // If column is not available, just print the raw output
            println!("{}", formatted);
        }
    }

    Ok(())
}

fn show_current() -> Result<()> {
    let config = Config::load().context("Failed to load configuration")?;

    println!("Current device: {}", config.device);
    let device_file = config.config_dir.join("device");
    if device_file.exists() {
        println!("Config file: {}", device_file.display());
    } else {
        println!("(using default)");
    }

    Ok(())
}

fn set_server(url_str: &str) -> Result<()> {
    // Parse and validate URL
    let url = Url::parse(url_str).with_context(|| format!("Invalid server URL: {}", url_str))?;

    // Validate URL scheme
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!(
            "Invalid URL scheme: {} (must be http or https)",
            url.scheme()
        );
    }

    // Validate URL has a host
    if url.host().is_none() {
        anyhow::bail!("Server URL must have a host");
    }

    // Load config, update server, and save
    let mut config = Config::load().context("Failed to load configuration")?;
    config.whisper_server = url;
    config.save().context("Failed to save configuration")?;

    println!("Server set to: {}", config.whisper_server);

    Ok(())
}

fn show_server() -> Result<()> {
    let config = Config::load().context("Failed to load configuration")?;

    println!("Current server: {}", config.whisper_server);
    let server_file = config.config_dir.join("server");
    if server_file.exists() {
        println!("Config file: {}", server_file.display());
    } else {
        println!("(using default)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_show_server_default() {
        let result = show_server();
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_and_show_server() {
        let temp_dir = TempDir::new().unwrap();
        std::env::set_var("HOME", temp_dir.path());

        let test_url = "http://localhost:9999";
        set_server(test_url).unwrap();

        let config = Config::load().unwrap();
        assert_eq!(config.whisper_server.as_str(), "http://localhost:9999/");
    }

    #[test]
    fn test_set_server_invalid_url() {
        let result = set_server("not-a-url");
        assert!(result.is_err());
    }

    #[test]
    fn test_set_server_invalid_scheme() {
        let result = set_server("ftp://localhost:8080");
        assert!(result.is_err());
    }

    #[test]
    fn test_show_current() {
        let result = show_current();
        assert!(result.is_ok());
    }
}
