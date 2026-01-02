//! ears - Speech recognition daemon CLI
//!
//! Main binary for the ears daemon

use anyhow::{Context, Result};
use clap::Parser;
use ears::{AudioFeedback, Config, DeviceManager, Notifications, Recorder, State, TextInput, WhisperClient};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ears")]
#[command(version)]
#[command(about = "Speech recognition daemon", long_about = None)]
struct Cli {
    /// Select audio device with fzf
    #[arg(short = 's', long = "select")]
    select: bool,

    /// List available audio devices
    #[arg(short = 'l', long = "list")]
    list: bool,

    /// Show current device configuration
    #[arg(short = 'c', long = "current")]
    current: bool,

    /// Show or set whisper server URL (provide URL to set, omit to show)
    #[arg(long = "server", value_name = "URL", num_args = 0..=1, default_missing_value = "")]
    server: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up debug logging to file
    let state = State::new()?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.log_file())
        .context("Failed to open log file")?;

    // Initialize tracing/logging to both file and env
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Arc::new(log_file))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    tracing::info!("ears started");

    let cli = Cli::parse();

    // Handle flags in order of precedence
    if cli.select {
        return handle_device_selection().await;
    }

    if cli.list {
        return handle_device_list().await;
    }

    if cli.current {
        return handle_show_current_device().await;
    }

    if let Some(url) = cli.server {
        return handle_server_config(&url).await;
    }

    // Main toggle logic (no flags provided)
    handle_toggle().await
}

/// Handle device selection with fzf
async fn handle_device_selection() -> Result<()> {
    let devices = DeviceManager::list_devices()?;

    if devices.is_empty() {
        println!("No audio input devices found");
        return Ok(());
    }

    if let Some(selected) = DeviceManager::select_device_interactive(&devices)? {
        Config::save_device(&selected)?;

        let device = devices
            .iter()
            .find(|d| d.name == selected)
            .context("Selected device not found")?;

        println!("Selected: {}", device.description);
        println!("Device ID: {}", device.name);
        println!("Saved to: {}/device", Config::config_dir()?.display());
    } else {
        println!("No device selected");
    }

    Ok(())
}

/// Handle device listing
async fn handle_device_list() -> Result<()> {
    let devices = DeviceManager::list_devices()?;

    if devices.is_empty() {
        println!("No audio input devices found");
        return Ok(());
    }

    // Print as a table
    for device in devices {
        println!("{}\t{}", device.name, device.description);
    }

    Ok(())
}

/// Handle showing current device
async fn handle_show_current_device() -> Result<()> {
    let device = Config::load_device()?;
    println!("Current device: {}", device);

    let config_file = Config::config_dir()?.join("device");
    if config_file.exists() {
        println!("Config file: {}", config_file.display());
    } else {
        println!("(using default)");
    }

    Ok(())
}

/// Handle server configuration
async fn handle_server_config(url: &str) -> Result<()> {
    if url.is_empty() {
        // Show current server
        let server = Config::load_server()?;
        println!("Current server: {}", server);

        let config_file = Config::config_dir()?.join("server");
        if config_file.exists() {
            println!("Config file: {}", config_file.display());
        } else {
            println!("(using default)");
        }
    } else {
        // Set server
        Config::save_server(url)?;
        println!("Server set to: {}", url);
    }

    Ok(())
}

/// Main toggle logic: start recording or stop and transcribe
async fn handle_toggle() -> Result<()> {
    let state = State::new()?;
    state.cleanup_stale()?;

    // Acquire lock to prevent concurrent execution
    // (Lock file handling would be done here in production)

    if state.is_recording() {
        stop_and_transcribe(&state).await
    } else {
        start_recording(&state).await
    }
}

/// Start a new recording
async fn start_recording(state: &State) -> Result<()> {
    // Load configuration
    let config = Config::load()?;

    // Check server health
    let client = WhisperClient::new(config.server.clone());
    if !client.health_check().await.unwrap_or(false) {
        AudioFeedback::beep_error().ok();
        Notifications::error("Whisper server not running!").ok();
        anyhow::bail!("Whisper server not available");
    }

    // Clean up any old audio file
    state.remove_audio().ok();

    // Start recording
    let recorder = Recorder::start(&config.device, &state.audio_file(), config.timeout)?;
    let pid = recorder.pid();

    // Save PID
    state.save_pid(pid as i32)?;

    // Play start beep
    AudioFeedback::beep_start().ok();

    tracing::info!("Recording started (PID: {})", pid);

    Ok(())
}

/// Stop recording and transcribe
async fn stop_and_transcribe(state: &State) -> Result<()> {
    let pid = state
        .get_recording_pid()
        .context("No recording PID found")?;

    // Check if process is still alive
    if unsafe { libc::kill(pid, 0) } != 0 {
        AudioFeedback::beep_error().ok();
        Notifications::info("No active recording").ok();
        state.cleanup_stale()?;
        return Ok(());
    }

    // Stop the recording process
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }

    // Remove PID file
    state.remove_pid()?;

    // Wait for file to be fully written
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Check if audio file exists and has content
    let audio_file = state.audio_file();
    if !audio_file.exists() {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is empty or missing").ok();
        anyhow::bail!("Audio file not found");
    }

    let metadata = tokio::fs::metadata(&audio_file).await?;
    if metadata.len() == 0 {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is empty").ok();
        state.remove_audio()?;
        anyhow::bail!("Audio file is empty");
    }

    // Load configuration
    let config = Config::load()?;

    // Transcribe
    let client = WhisperClient::new(config.server);
    match client.transcribe(&audio_file).await {
        Ok(text) if !text.is_empty() => {
            // Type the text
            TextInput::type_text(&text)?;

            // Play success beep
            AudioFeedback::beep_done().ok();

            tracing::info!("Transcription successful: {}", text);
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
            return Err(e);
        }
    }

    // Clean up audio file
    state.remove_audio()?;

    Ok(())
}
