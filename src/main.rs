mod cli;
mod recording;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands, DeviceAction};
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
    // Parse CLI first (need profile for config loading)
    let cli = Cli::parse();

    // Load config with profile
    let config = Config::load_profile(cli.profile.as_deref()).unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load config: {}", e);
        Config::new().expect("Failed to create config")
    });

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
    tracing::info!("Log file: {}", log_file_path.display());

    match cli.command {
        Some(Commands::Toggle) => {
            handle_toggle(&config).await?;
        }
        Some(Commands::Vad) => {
            handle_vad(&config).await?;
        }
        Some(Commands::Device { action }) => match action {
            Some(DeviceAction::List) => list_devices()?,
            Some(DeviceAction::Select) => select_device(&config)?,
            Some(DeviceAction::Current) | None => show_current(&config)?,
        },
        Some(Commands::Profile { name }) => {
            if let Some(name) = name {
                set_profile(&name)?;
            } else {
                show_profile()?;
            }
        }
        Some(Commands::Server { url }) => {
            if let Some(url_str) = url {
                set_server(&config, &url_str)?;
            } else {
                show_server(&config)?;
            }
        }
        // Hidden backwards-compat aliases
        Some(Commands::Select) => select_device(&config)?,
        Some(Commands::List) => list_devices()?,
        Some(Commands::Current) => show_current(&config)?,
        None => {
            // Default: Launch TUI
            return ears::tui::run(cli.profile.as_deref()).await;
        }
    }

    Ok(())
}

fn select_device(config: &Config) -> Result<()> {
    let devices = audio::list_devices().context("Failed to list audio devices")?;

    if devices.is_empty() {
        eprintln!("No audio input devices found");
        anyhow::bail!("No audio input devices available");
    }

    let selected =
        audio::select_device_interactive(&devices).context("Failed to run device selection")?;

    let device_name = match selected {
        Some(name) => name,
        None => {
            eprintln!("No device selected");
            return Ok(());
        }
    };

    let device = devices
        .iter()
        .find(|d| d.name == device_name)
        .context("Selected device not found in list")?;

    let mut config = config.clone();
    config.device = device_name.clone();
    config.save().context("Failed to save configuration")?;

    println!("Selected: {}", device.description);
    println!("Device ID: {}", device_name);
    println!(
        "Saved to: {}",
        config.config_dir.join("config.toml").display()
    );

    Ok(())
}

fn list_devices() -> Result<()> {
    let devices = audio::list_devices().context("Failed to list audio devices")?;

    if devices.is_empty() {
        println!("No audio input devices found");
        return Ok(());
    }

    let formatted = audio::format_device_list(&devices);

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
            println!("{}", formatted);
        }
    }

    Ok(())
}

fn show_current(config: &Config) -> Result<()> {
    println!("Current device: {}", config.device);
    println!(
        "Config: {}",
        config.config_dir.join("config.toml").display()
    );

    Ok(())
}

fn set_server(config: &Config, url_str: &str) -> Result<()> {
    let url = Url::parse(url_str).with_context(|| format!("Invalid server URL: {}", url_str))?;

    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!(
            "Invalid URL scheme: {} (must be http or https)",
            url.scheme()
        );
    }

    if url.host().is_none() {
        anyhow::bail!("Server URL must have a host");
    }

    let mut config = config.clone();
    config.whisper_server = url;
    config.save().context("Failed to save configuration")?;

    println!("Server set to: {}", config.whisper_server);

    Ok(())
}

fn show_profile() -> Result<()> {
    let current = Config::get_default_profile()?;
    match current {
        Some(ref name) => println!("Default profile: {}", name),
        None => println!("No default profile set (using config.toml)"),
    }

    let profiles = Config::list_profiles()?;
    if profiles.is_empty() {
        println!("\nNo named profiles found.");
        println!("Create one at ~/.config/ears/config.<name>.toml");
    } else {
        println!("\nAvailable profiles:");
        for p in &profiles {
            let marker = if current.as_deref() == Some(p) {
                " *"
            } else {
                ""
            };
            println!("  {}{}", p, marker);
        }
    }

    Ok(())
}

fn set_profile(name: &str) -> Result<()> {
    if name == "default" || name.is_empty() {
        Config::set_default_profile("")?;
        println!("Profile cleared (using config.toml)");
    } else {
        Config::set_default_profile(name)?;
        println!("Default profile set to: {}", name);
    }
    Ok(())
}

fn show_server(config: &Config) -> Result<()> {
    println!("Current server: {}", config.whisper_server);
    println!(
        "Config: {}",
        config.config_dir.join("config.toml").display()
    );

    Ok(())
}

/// Run post-transcribe hook if it exists (fire-and-forget)
fn run_post_transcribe_hook(audio_file: &std::path::Path, text: &str) {
    use directories::ProjectDirs;
    use std::os::unix::fs::PermissionsExt;

    let hook_path = ProjectDirs::from("com", "heiervang", "ears")
        .map(|p| p.config_dir().join("hooks/post-transcribe"))
        .unwrap_or_default();

    let is_executable = std::fs::metadata(&hook_path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false);

    if !is_executable {
        return;
    }

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

    std::thread::spawn(move || {
        let mut command = std::process::Command::new(&hook_path);
        command.arg(&hook_audio)
            .arg(&text_owned)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        match command.spawn() {
            Ok(mut child) => {
                tracing::info!("Post-transcribe hook started");
                let _ = child.wait();
            }
            Err(e) => tracing::warn!("Failed to run post-transcribe hook: {}", e),
        }
    });
}

/// Toggle VAD mode: start or stop headless voice activity detection
async fn handle_vad(config: &Config) -> Result<()> {
    let vad_pid_file = config.state_dir.join("vad.pid");

    if let Ok(pid_str) = std::fs::read_to_string(&vad_pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid), None).is_ok() {
                kill(Pid::from_raw(pid), Signal::SIGTERM).ok();
                eprintln!("VAD stopped");
                return Ok(());
            }
        }
        std::fs::remove_file(&vad_pid_file).ok();
    }

    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;
    state_mgr.load_state().ok();

    std::fs::write(&vad_pid_file, std::process::id().to_string())
        .context("Failed to write VAD PID file")?;

    if state_mgr.current_state() != StateEnum::Idle {
        state_mgr.transition(StateEnum::Idle).ok();
    }
    state_mgr
        .transition(StateEnum::VadActive)
        .context("Failed to transition to VadActive")?;

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ears::streaming_engine::StreamingEvent>();

    let (shutdown_tx, settings_tx, pipeline_handle) =
        match ears::tui::start_vad_pipeline(config, event_tx).await {
            Ok(result) => result,
            Err(e) => {
                std::fs::remove_file(&vad_pid_file).ok();
                state_mgr.transition(StateEnum::Idle).ok();
                anyhow::bail!("Failed to start VAD: {}", e);
            }
        };

    // Send config-driven settings to the engine
    let _ = settings_tx.send(ears::tui::TypingSettings {
        progressive_typing: true,
        auto_correction: true,
        typing_mode: config.typing_mode,
        auto_enter: config.auto_enter,
    });

    eprintln!("VAD started - listening...");

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

    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = sigterm.recv() => {}
        _ = sigint.recv() => {}
    }

    let _ = shutdown_tx.send(true);
    let _ = pipeline_handle.await;
    std::fs::remove_file(&vad_pid_file).ok();
    state_mgr.transition(StateEnum::Idle).ok();

    Ok(())
}

/// Main toggle logic: start recording or stop and transcribe
async fn handle_toggle(config: &Config) -> Result<()> {
    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;

    state_mgr.load_state().context("Failed to load state")?;

    let pid_file = config.state_dir.join("recording.pid");
    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(120));

    process_mgr.cleanup_stale().ok();

    state_mgr
        .reconcile_state(|| {
            process_mgr
                .is_recording_alive()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        })
        .context("Failed to reconcile state")?;

    let is_recording = process_mgr.is_recording_alive().unwrap_or(false);

    if is_recording {
        stop_and_transcribe(config, &mut state_mgr, &process_mgr).await
    } else {
        start_recording(config, &mut state_mgr, &process_mgr).await
    }
}

/// Start a new recording
async fn start_recording(
    config: &Config,
    state_mgr: &mut StateManager,
    process_mgr: &ProcessManager,
) -> Result<()> {
    let toggle_start = std::time::Instant::now();
    tracing::info!("Starting recording");

    let health_start = std::time::Instant::now();
    let client = WhisperClient::new(config.whisper_server.clone())
        .with_api_key(config.api_key.clone())
        .with_model(config.model.clone());
    if client.health_check().await.is_err() {
        tracing::error!("Whisper server health check failed");
        AudioFeedback::beep_error().ok();
        Notifications::error("Whisper server not running!").ok();
        anyhow::bail!("Whisper server not available");
    }

    tracing::info!("Health check passed in {:?}", health_start.elapsed());

    let audio_file = config.state_dir.join("recording.wav");
    if audio_file.exists() {
        std::fs::remove_file(&audio_file).ok();
    }

    state_mgr
        .transition(StateEnum::Recording)
        .context("Failed to transition to Recording state")?;

    let pid = process_mgr
        .spawn_recording(&config.device, &audio_file)
        .context("Failed to start recording")?;

    AudioFeedback::beep_start().ok();

    tracing::info!(
        "Recording started (PID: {}) total start_recording: {:?}",
        pid,
        toggle_start.elapsed()
    );

    Ok(())
}

/// Stop recording and transcribe
async fn stop_and_transcribe(
    config: &Config,
    state_mgr: &mut StateManager,
    process_mgr: &ProcessManager,
) -> Result<()> {
    let total_start = std::time::Instant::now();
    tracing::info!("Stopping recording and transcribing");

    let pid = match process_mgr.read_pid()? {
        Some(pid) => pid,
        None => {
            AudioFeedback::beep_error().ok();
            Notifications::info("No active recording").ok();
            tracing::warn!("No recording PID found");
            return Ok(());
        }
    };

    if !process_mgr.is_process_alive(pid) {
        AudioFeedback::beep_error().ok();
        Notifications::info("No active recording").ok();
        process_mgr.cleanup_stale().ok();
        tracing::warn!("Recording process not alive (PID: {})", pid);
        return Ok(());
    }

    let stop_start = std::time::Instant::now();
    process_mgr
        .stop_recording()
        .context("Failed to stop recording")?;

    tokio::time::sleep(Duration::from_millis(300)).await;
    tracing::info!("Recording stopped in {:?}", stop_start.elapsed());

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

    let mut header = [0u8; 12];
    let mut file = tokio::fs::File::open(&audio_file).await?;
    use tokio::io::AsyncReadExt;
    file.read_exact(&mut header).await?;

    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is corrupted").ok();
        std::fs::remove_file(&audio_file).ok();
        tracing::error!("Audio file is not a valid WAV file");
        anyhow::bail!("Invalid audio file format");
    }

    tracing::info!("WAV file validation passed");

    state_mgr
        .transition(StateEnum::Transcribing)
        .context("Failed to transition to Transcribing state")?;

    let lang_start = std::time::Instant::now();
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    tracing::info!("Language detection took {:?}", lang_start.elapsed());
    if let Some(ref lang) = language {
        tracing::info!("Using language: {} (from keyboard layout)", lang);
    }

    let transcribe_start = std::time::Instant::now();
    let client = WhisperClient::new(config.whisper_server.clone())
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(config.model.clone());
    match client.transcribe(&audio_file).await {
        Ok(text) if !text.is_empty() => {
            tracing::info!(
                "Transcription completed in {:?}: {}",
                transcribe_start.elapsed(),
                text
            );

            let filtered_text = config.text_filters.apply(&text, language.as_deref());
            tracing::debug!("Filtered text: {}", filtered_text);

            let typing_start = std::time::Instant::now();
            match TextInput::type_text(&filtered_text, config.typing_mode) {
                Ok(()) => {
                    if config.auto_enter {
                        if let Err(e) = TextInput::send_enter() {
                            tracing::warn!("Failed to send Enter key: {}", e);
                        }
                    }
                    tracing::info!(
                        "Text typing completed in {:?} ({} chars)",
                        typing_start.elapsed(),
                        filtered_text.len()
                    );
                    AudioFeedback::beep_done().ok();
                }
                Err(e) => {
                    tracing::error!("Failed to type text in {:?}: {}", typing_start.elapsed(), e);
                    AudioFeedback::beep_error().ok();
                    Notifications::error(&format!("Failed to type text: {}", e)).ok();
                }
            }

            run_post_transcribe_hook(&audio_file, &filtered_text);
        }
        Ok(_) => {
            AudioFeedback::beep_error().ok();
            Notifications::info("No speech detected").ok();
            tracing::info!("No speech detected");

            state_mgr.transition(StateEnum::Idle).ok();
        }
        Err(e) => {
            AudioFeedback::beep_error().ok();
            Notifications::error(&format!("Transcription failed: {}", e)).ok();
            tracing::error!("Transcription failed: {}", e);

            std::fs::remove_file(&audio_file).ok();

            state_mgr.transition(StateEnum::Idle).ok();

            return Err(e.into());
        }
    }

    std::fs::remove_file(&audio_file).ok();

    state_mgr
        .transition(StateEnum::Idle)
        .context("Failed to transition to Idle state")?;

    tracing::info!("Total stop_and_transcribe: {:?}", total_start.elapsed());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_server_validation() {
        // Test URL validation without calling Config::load/save
        let test_url = "http://localhost:9999";
        let parsed = Url::parse(test_url).unwrap();
        assert_eq!(parsed.scheme(), "http");
        assert_eq!(parsed.host_str(), Some("localhost"));
        assert_eq!(parsed.port(), Some(9999));
        assert_eq!(parsed.as_str(), "http://localhost:9999/");
    }

    #[test]
    fn test_invalid_url() {
        assert!(Url::parse("not-a-url").is_err());
    }

    #[test]
    fn test_invalid_scheme() {
        let url = Url::parse("ftp://localhost:8080").unwrap();
        assert!(!matches!(url.scheme(), "http" | "https"));
    }
}
