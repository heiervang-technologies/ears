mod audio;
mod cli;
mod config;
mod recording;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands};
use config::Config;
use ears::{
    AudioFeedback, Notifications, ProcessManager, State as StateEnum, StateManager, TextInput,
    WhisperClient,
};
use std::time::Duration;
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing/logging
    let config = Config::load().unwrap_or_else(|_| Config::new().expect("Failed to create config"));

    // Set up debug logging to file
    let log_file_path = config.state_dir.join("debug.log");
    if let Ok(log_file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
    {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Arc::new(log_file))
            .with_ansi(false)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();
    }

    tracing::info!("ears started");

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
            // Main toggle logic
            handle_toggle().await?;
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

/// Main toggle logic: start recording or stop and transcribe
async fn handle_toggle() -> Result<()> {
    let config = Config::load().context("Failed to load configuration")?;

    // Create state manager
    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;

    // Load current state from disk
    state_mgr.load_state().context("Failed to load state")?;

    // Create process manager
    let pid_file = config.state_dir.join("recording.pid");
    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(120));

    // Clean up any stale state
    process_mgr.cleanup_stale().ok();

    // Check if we're currently recording
    let is_recording = process_mgr.is_recording_alive().unwrap_or(false);

    if is_recording {
        stop_and_transcribe(&config, &process_mgr).await
    } else {
        start_recording(&config, &mut state_mgr, &process_mgr).await
    }
}

/// Start a new recording
async fn start_recording(
    config: &Config,
    state_mgr: &mut StateManager,
    process_mgr: &ProcessManager,
) -> Result<()> {
    tracing::info!("Starting recording");

    // Check server health
    let client = WhisperClient::new(config.whisper_server.clone());
    if client.health_check().await.is_err() {
        tracing::error!("Whisper server health check failed");
        AudioFeedback::beep_error().ok();
        Notifications::error("Whisper server not running!").ok();
        anyhow::bail!("Whisper server not available");
    }

    // Clean up any old audio file
    let audio_file = config.state_dir.join("recording.wav");
    if audio_file.exists() {
        std::fs::remove_file(&audio_file).ok();
    }

    // Transition to Recording state
    state_mgr
        .transition(StateEnum::Recording)
        .context("Failed to transition to Recording state")?;

    // Start recording
    let pid = process_mgr
        .spawn_recording(&config.device, &audio_file)
        .context("Failed to start recording")?;

    // Play start beep
    AudioFeedback::beep_start().ok();

    tracing::info!("Recording started (PID: {})", pid);

    Ok(())
}

/// Stop recording and transcribe
async fn stop_and_transcribe(config: &Config, process_mgr: &ProcessManager) -> Result<()> {
    tracing::info!("Stopping recording and transcribing");

    // Get the PID
    let pid = match process_mgr.read_pid()? {
        Some(pid) => pid,
        None => {
            AudioFeedback::beep_error().ok();
            Notifications::info("No active recording").ok();
            tracing::warn!("No recording PID found");
            return Ok(());
        }
    };

    // Check if process is still alive
    if !process_mgr.is_process_alive(pid) {
        AudioFeedback::beep_error().ok();
        Notifications::info("No active recording").ok();
        process_mgr.cleanup_stale().ok();
        tracing::warn!("Recording process not alive (PID: {})", pid);
        return Ok(());
    }

    // Stop the recording process
    process_mgr
        .stop_recording()
        .context("Failed to stop recording")?;

    // Wait for file to be fully written
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Check if audio file exists and has content
    let audio_file = config.state_dir.join("recording.wav");
    if !audio_file.exists() {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is empty or missing").ok();
        tracing::error!("Audio file not found: {}", audio_file.display());
        anyhow::bail!("Audio file not found");
    }

    let metadata = tokio::fs::metadata(&audio_file).await?;
    if metadata.len() == 0 {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is empty").ok();
        std::fs::remove_file(&audio_file).ok();
        tracing::error!("Audio file is empty");
        anyhow::bail!("Audio file is empty");
    }

    tracing::info!("Audio file size: {} bytes", metadata.len());

    // Transcribe
    let client = WhisperClient::new(config.whisper_server.clone());
    match client.transcribe(&audio_file).await {
        Ok(text) if !text.is_empty() => {
            tracing::info!("Transcription successful: {}", text);

            // Type the text
            TextInput::type_text(&text)?;

            // Play success beep
            AudioFeedback::beep_done().ok();
        }
        Ok(_) => {
            // Empty transcription (silence)
            AudioFeedback::beep_error().ok();
            Notifications::info("No speech detected").ok();
            tracing::info!("No speech detected");
        }
        Err(e) => {
            AudioFeedback::beep_error().ok();
            Notifications::error(&format!("Transcription failed: {}", e)).ok();
            tracing::error!("Transcription failed: {}", e);

            // Clean up audio file
            std::fs::remove_file(&audio_file).ok();

            return Err(e.into());
        }
    }

    // Clean up audio file
    std::fs::remove_file(&audio_file).ok();

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
