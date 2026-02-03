mod cli;
mod recording;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands};
use ears::audio;
use ears::Config;
use ears::{
    AudioFeedback, KeyboardLayout, Notifications, ProcessManager, State as StateEnum, StateManager,
    TextInput, WhisperClient,
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

    match cli.command {
        Some(Commands::Toggle) => {
            // Toggle recording/transcription (for keyboard shortcuts)
            handle_toggle().await?;
        }
        Some(Commands::Vad) => {
            handle_vad().await?;
        }
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
            // Default: Launch TUI
            return ears::tui::run().await;
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

/// Run post-transcribe hook if it exists (fire-and-forget)
fn run_post_transcribe_hook(audio_file: &std::path::Path, text: &str) {
    use directories::ProjectDirs;
    use std::os::unix::fs::PermissionsExt;

    let hook_path = ProjectDirs::from("com", "heiervang", "ears")
        .map(|p| p.config_dir().join("hooks/post-transcribe"))
        .unwrap_or_default();

    // Check if hook exists and is executable
    let is_executable = std::fs::metadata(&hook_path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false);

    if !is_executable {
        return;
    }

    // Copy audio file for the hook (it runs async, original gets deleted)
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let hook_audio = audio_file
        .parent()
        .unwrap_or(std::path::Path::new("/tmp"))
        .join(format!("hook-{}.wav", timestamp));

    if std::fs::copy(audio_file, &hook_audio).is_err() {
        tracing::warn!("Failed to copy audio file for hook");
        return;
    }

    let text_owned = text.to_string();

    // Spawn hook in background - fire and forget
    std::thread::spawn(move || {
        let result = std::process::Command::new(&hook_path)
            .arg(&hook_audio)
            .arg(&text_owned)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match result {
            Ok(_) => tracing::info!("Post-transcribe hook started"),
            Err(e) => tracing::warn!("Failed to run post-transcribe hook: {}", e),
        }
    });
}

/// Toggle VAD mode: start or stop headless voice activity detection
async fn handle_vad() -> Result<()> {
    let config = Config::load().context("Failed to load configuration")?;
    let vad_pid_file = config.state_dir.join("vad.pid");

    // Check if a headless VAD process is already running
    if let Ok(pid_str) = std::fs::read_to_string(&vad_pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid), None).is_ok() {
                // Process is alive, send SIGTERM to stop it
                kill(Pid::from_raw(pid), Signal::SIGTERM).ok();
                eprintln!("VAD stopped");
                return Ok(());
            }
        }
        // Stale PID file
        std::fs::remove_file(&vad_pid_file).ok();
    }

    // Start VAD mode
    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;
    state_mgr.load_state().ok();

    // Write our PID
    std::fs::write(&vad_pid_file, std::process::id().to_string())
        .context("Failed to write VAD PID file")?;

    // Force state to Idle if stale, then transition to VadActive
    if state_mgr.current_state() != StateEnum::Idle {
        state_mgr.transition(StateEnum::Idle).ok();
    }
    state_mgr
        .transition(StateEnum::VadActive)
        .context("Failed to transition to VadActive")?;

    // Create event channel
    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ears::streaming_engine::StreamingEvent>();

    // Start pipeline
    let (shutdown_tx, _settings_tx, pipeline_handle) =
        match ears::tui::start_vad_pipeline(&config, event_tx).await {
            Ok(result) => result,
            Err(e) => {
                std::fs::remove_file(&vad_pid_file).ok();
                state_mgr.transition(StateEnum::Idle).ok();
                anyhow::bail!("Failed to start VAD: {}", e);
            }
        };

    eprintln!("VAD started - listening...");

    // Drain events in background (log segment completions and errors)
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                ears::streaming_engine::StreamingEvent::SegmentCompleted { text, duration_ms } => {
                    tracing::info!("Segment: \"{}\" ({}ms)", text, duration_ms);
                }
                ears::streaming_engine::StreamingEvent::Error(msg) => {
                    tracing::warn!("Streaming error: {}", msg);
                }
                _ => {}
            }
        }
    });

    // Wait for termination signal (SIGTERM from second `ears vad`, or Ctrl+C)
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = sigterm.recv() => {}
        _ = sigint.recv() => {}
    }

    // Shutdown
    let _ = shutdown_tx.send(true);
    let _ = pipeline_handle.await;
    std::fs::remove_file(&vad_pid_file).ok();
    state_mgr.transition(StateEnum::Idle).ok();

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

    // Clean up any stale PID files
    process_mgr.cleanup_stale().ok();

    // Reconcile state with actual process status (fixes Issue #52)
    // If state says Recording but no process is running, reset to Idle
    state_mgr
        .reconcile_state(|| {
            process_mgr
                .is_recording_alive()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        })
        .context("Failed to reconcile state")?;

    // Check if we're currently recording
    let is_recording = process_mgr.is_recording_alive().unwrap_or(false);

    if is_recording {
        stop_and_transcribe(&config, &mut state_mgr, &process_mgr).await
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
async fn stop_and_transcribe(
    config: &Config,
    state_mgr: &mut StateManager,
    process_mgr: &ProcessManager,
) -> Result<()> {
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

    // Validate WAV file format
    let mut header = [0u8; 12];
    let mut file = tokio::fs::File::open(&audio_file).await?;
    use tokio::io::AsyncReadExt;
    file.read_exact(&mut header).await?;

    // Check for "RIFF" signature and "WAVE" format
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is corrupted").ok();
        std::fs::remove_file(&audio_file).ok();
        tracing::error!("Audio file is not a valid WAV file");
        anyhow::bail!("Invalid audio file format");
    }

    tracing::info!("WAV file validation passed");

    // Transition to transcribing state
    state_mgr
        .transition(StateEnum::Transcribing)
        .context("Failed to transition to Transcribing state")?;

    // Detect language from keyboard layout (overrides config if set)
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    if let Some(ref lang) = language {
        tracing::info!("Using language: {} (from keyboard layout)", lang);
    }

    // Transcribe
    let client = WhisperClient::new(config.whisper_server.clone()).with_language(language);
    match client.transcribe(&audio_file).await {
        Ok(text) if !text.is_empty() => {
            tracing::info!("Transcription successful: {}", text);

            // Apply text filters (lowercase, punctuation removal, etc.)
            let filtered_text = config.text_filters.apply(&text);
            tracing::debug!("Filtered text: {}", filtered_text);

            // Type the text (don't bail on failure — state cleanup must happen)
            match TextInput::type_text(&filtered_text) {
                Ok(()) => {
                    AudioFeedback::beep_done().ok();
                }
                Err(e) => {
                    tracing::error!("Failed to type text: {}", e);
                    AudioFeedback::beep_error().ok();
                    Notifications::error(&format!("Failed to type text: {}", e)).ok();
                }
            }

            // Run post-transcribe hook if it exists (with filtered text)
            run_post_transcribe_hook(&audio_file, &filtered_text);
        }
        Ok(_) => {
            // Empty transcription (silence)
            AudioFeedback::beep_error().ok();
            Notifications::info("No speech detected").ok();
            tracing::info!("No speech detected");

            // Reset state to Idle on empty transcription
            state_mgr.transition(StateEnum::Idle).ok();
        }
        Err(e) => {
            AudioFeedback::beep_error().ok();
            Notifications::error(&format!("Transcription failed: {}", e)).ok();
            tracing::error!("Transcription failed: {}", e);

            // Clean up audio file
            std::fs::remove_file(&audio_file).ok();

            // Reset state to Idle on error
            state_mgr.transition(StateEnum::Idle).ok();

            return Err(e.into());
        }
    }

    // Clean up audio file
    std::fs::remove_file(&audio_file).ok();

    // Reset state to Idle
    state_mgr
        .transition(StateEnum::Idle)
        .context("Failed to transition to Idle state")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    #[serial_test::serial]
    fn test_show_server_default() {
        // Use temp HOME so Config::load() doesn't read/write real ~/.config/ears/
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());

        let result = show_server();

        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        assert!(result.is_ok());
    }

    #[test]
    fn test_set_and_show_server() {
        // Test URL validation without calling Config::load/save
        let test_url = "http://localhost:9999";
        let parsed = Url::parse(test_url).unwrap();
        assert_eq!(parsed.scheme(), "http");
        assert_eq!(parsed.host_str(), Some("localhost"));
        assert_eq!(parsed.port(), Some(9999));
        assert_eq!(parsed.as_str(), "http://localhost:9999/");

        // Verify set_server rejects bad URLs (these fail before reaching Config::load)
        assert!(set_server("not-a-url").is_err());
        assert!(set_server("ftp://localhost:8080").is_err());
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
    #[serial_test::serial]
    fn test_show_current() {
        // Use temp HOME so Config::load() doesn't read/write real ~/.config/ears/
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", temp_dir.path());

        let result = show_current();

        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        assert!(result.is_ok());
    }
}
