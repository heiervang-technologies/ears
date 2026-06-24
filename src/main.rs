mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands, DeviceAction};
use ears::audio;
use ears::Config;
use ears::{
    AudioFeedback, FileLock, KeyboardLayout, Notifications, ProcessManager, State as StateEnum,
    StateManager, TextInput, WhisperClient,
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

    // Set audio cue volume from config
    AudioFeedback::set_volume(config.cue_volume);

    match cli.command {
        Some(Commands::Toggle) => {
            handle_toggle(&config).await?;
        }
        Some(Commands::Vad) => {
            handle_vad(&config).await?;
        }
        Some(Commands::WsListen { host, port, socket }) => {
            handle_ws_listen(&config, &host, port, socket).await?;
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
        Some(Commands::Test { file }) => {
            test_config(&config, file.as_deref()).await?;
        }
        Some(Commands::AutoEnter) => {
            handle_auto_enter().await?;
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

async fn handle_auto_enter() -> Result<()> {
    match ears::ipc::send_command("toggle-auto-enter").await {
        Ok(resp) => {
            eprintln!("{}", resp);
            Ok(())
        }
        Err(_) => {
            anyhow::bail!("No running ears instance found (is VAD or TUI running?)");
        }
    }
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
    println!("Saved to: {}", config.config_file().display());

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
    println!("Config: {}", config.config_file().display());

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
    println!("Config: {}", config.config_file().display());

    Ok(())
}

/// Mask a secret for display: keep a short prefix/suffix, hide the middle.
fn mask_secret(secret: &str) -> String {
    let len = secret.chars().count();
    if len <= 8 {
        return "set (hidden)".to_string();
    }
    let prefix: String = secret.chars().take(4).collect();
    let suffix: String = secret.chars().skip(len - 3).collect();
    format!("set ({}…{})", prefix, suffix)
}

/// Validate the active configuration: print a summary, check server health, and
/// optionally transcribe a sample audio file. Exits non-zero on failure.
async fn test_config(config: &Config, file: Option<&str>) -> Result<()> {
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());

    println!("Config:   {}", config.config_file().display());
    println!(
        "Profile:  {}",
        config.active_profile.as_deref().unwrap_or("default")
    );
    println!("Server:   {}", server_url);
    // Mirror the endpoint WhisperClient actually builds (trim trailing '/').
    println!(
        "Endpoint: {}/v1/audio/transcriptions",
        server_url.as_str().trim_end_matches('/')
    );
    println!(
        "Model:    {}",
        model.as_deref().unwrap_or("(server default)")
    );
    println!("Device:   {}", config.device);
    println!(
        "Language: {}",
        language.as_deref().unwrap_or("(auto-detect)")
    );
    println!(
        "API key:  {}",
        config
            .api_key
            .as_deref()
            .map(mask_secret)
            .unwrap_or_else(|| "(none)".to_string())
    );

    if config.server_has_redundant_v1() {
        println!();
        println!(
            "⚠  Server URL ends in /v1. ears appends /v1/audio/transcriptions itself, so\n   \
             requests hit a doubled /v1/v1 path and will fail. Drop the trailing /v1."
        );
    }

    println!();
    print!("Health check… ");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let client = WhisperClient::new(server_url.to_string())
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(model)
        .with_prompt(config.prompt.clone());

    if let Err(e) = client.health_check().await {
        println!("FAILED");
        anyhow::bail!("Server health check failed: {}", e);
    }
    println!("OK");

    if let Some(path) = file {
        print!("Transcribing {}… ", path);
        std::io::stdout().flush().ok();
        match client.transcribe(path).await {
            Ok(text) => {
                println!("OK");
                println!("Transcript: {}", text);
            }
            Err(e) => {
                println!("FAILED");
                anyhow::bail!("Transcription failed: {}", e);
            }
        }
    }

    println!();
    println!("✓ Config OK");
    Ok(())
}

// Use shared StateResetGuard from ears::state

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
        command
            .arg(&hook_audio)
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
    AudioFeedback::set_volume(config.cue_volume);
    let vad_pid_file = config.state_dir.join("vad.pid");
    let vad_lock_path = config.state_dir.join("vad.lock");
    let mut vad_lock = FileLock::new(&vad_lock_path).context("Failed to create VAD lock")?;
    if !vad_lock.try_lock().context("Failed to check VAD lock")? {
        // Another VAD instance is starting up — just try to kill it via PID
        tracing::info!("VAD lock held by another process, attempting stop via PID");
    }

    if let Ok(pid_str) = std::fs::read_to_string(&vad_pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid), None).is_ok() {
                if let Err(e) = kill(Pid::from_raw(pid), Signal::SIGTERM) {
                    tracing::warn!("Failed to send SIGTERM to VAD process {}: {}", pid, e);
                }
                AudioFeedback::beep_vad_close().ok();
                eprintln!("VAD stopped");
                return Ok(());
            }
        }
        if let Err(e) = std::fs::remove_file(&vad_pid_file) {
            tracing::debug!("Failed to remove stale VAD PID file: {}", e);
        }
    }

    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;
    if let Err(e) = state_mgr.load_state() {
        tracing::warn!("Failed to load state: {}", e);
    }

    std::fs::write(&vad_pid_file, std::process::id().to_string())
        .context("Failed to write VAD PID file")?;

    if state_mgr.current_state() != StateEnum::Idle {
        if let Err(e) = state_mgr.transition(StateEnum::Idle) {
            tracing::warn!("Failed to reset state to Idle: {}", e);
        }
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
                if let Err(e2) = std::fs::remove_file(&vad_pid_file) {
                    tracing::debug!("Failed to remove VAD PID file: {}", e2);
                }
                if let Err(e2) = state_mgr.transition(StateEnum::Idle) {
                    tracing::warn!("Failed to reset state after VAD failure: {}", e2);
                }
                anyhow::bail!("Failed to start VAD: {}", e);
            }
        };

    // Helper to build typing settings from current config state
    let make_settings = |auto_enter: bool| {
        let language = ears::KeyboardLayout::detect_language().or_else(|| config.language.clone());
        ears::tui::TypingSettings {
            progressive_typing: true,
            auto_correction: true,
            typing_mode: config.typing_mode,
            auto_enter,
            text_filters: config.text_filters.clone(),
            language,
        }
    };

    // Send config-driven settings to the engine
    let _ = settings_tx.send(make_settings(config.auto_enter));

    AudioFeedback::beep_vad_open().ok();
    eprintln!("VAD started - listening...");

    let (ipc_tx, ipc_rx) = tokio::sync::broadcast::channel(100);
    ears::ipc::start_ipc_server(ipc_rx);

    // Start command server for remote control (e.g. `ears auto-enter`)
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    ears::ipc::start_cmd_server(cmd_tx);

    // Track mutable settings for command updates
    let mut auto_enter = config.auto_enter;

    let save_to_clipboard = config.save_to_clipboard;
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let _ = ipc_tx.send(event.clone());
            match event {
                ears::streaming_engine::StreamingEvent::SpeechProbable => {
                    AudioFeedback::beep_vad_speech_start().ok();
                }
                ears::streaming_engine::StreamingEvent::SpeechStarted => {
                    AudioFeedback::beep_vad_speech_confirm().ok();
                }
                ears::streaming_engine::StreamingEvent::SpeechEnded => {
                    AudioFeedback::beep_vad_end().ok();
                }
                ears::streaming_engine::StreamingEvent::SegmentCompleted { text, duration_ms } => {
                    tracing::info!("Segment: \"{}\" ({}ms)", text, duration_ms);
                    if save_to_clipboard && !text.is_empty() {
                        TextInput::copy_to_clipboard(&text);
                    }
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

    loop {
        tokio::select! {
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    ears::ipc::EarsCommand::ToggleAutoEnter { respond } => {
                        auto_enter = !auto_enter;
                        let _ = settings_tx.send(make_settings(auto_enter));
                        if auto_enter {
                            AudioFeedback::beep_toggle_on().ok();
                        } else {
                            AudioFeedback::beep_toggle_off().ok();
                        }
                        let state = if auto_enter { "on" } else { "off" };
                        let _ = respond.send(format!("auto-enter:{}", state));
                        tracing::info!("Auto-enter toggled to {}", auto_enter);
                        eprintln!("Auto-enter: {}", state);
                    }
                }
            }
        }
    }

    let _ = shutdown_tx.send(true);
    let _ = pipeline_handle.await;
    ears::ipc::cleanup_socket();
    ears::ipc::cleanup_cmd_socket();
    if let Err(e) = std::fs::remove_file(&vad_pid_file) {
        tracing::debug!("Failed to remove VAD PID file: {}", e);
    }
    if let Err(e) = state_mgr.transition(StateEnum::Idle) {
        tracing::warn!("Failed to transition to Idle on shutdown: {}", e);
    }

    Ok(())
}

/// Run WebSocket audio input mode: starts a WS server that feeds audio into the VAD pipeline
async fn handle_ws_listen(
    config: &Config,
    host: &str,
    port: u16,
    socket: Option<String>,
) -> Result<()> {
    // Use a custom socket path to avoid conflicting with the desktop ears instance
    let socket_path = socket.map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::var("XDG_RUNTIME_DIR")
            .map(|d| std::path::PathBuf::from(d).join("ears-ws.sock"))
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/ears-ws.sock"))
    });

    // Do NOT kill existing VAD — ws-listen runs alongside the desktop ears instance

    // Build the VAD pipeline components manually (like start_vad_pipeline but without pw-record)
    let language = ears::KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());
    let whisper_client = std::sync::Arc::new(
        ears::WhisperClient::new(server_url.to_string())
            .with_language(language)
            .with_api_key(config.api_key.clone())
            .with_model(model)
            .with_prompt(config.prompt.clone()),
    );

    whisper_client
        .health_check()
        .await
        .map_err(|e| anyhow::anyhow!("Whisper server health check failed: {}", e))?;

    // Audio channel — WebSocket server writes here, engine reads
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();

    // Broadcast channel for streaming events — shared by IPC server and WebSocket echo
    let (ipc_tx, ipc_rx) = tokio::sync::broadcast::channel(100);

    // Start WebSocket server (echoes events back to connected clients)
    let ws_handle = ears::ws_input::start_ws_server(host, port, audio_tx, ipc_tx.clone()).await?;

    // Create streaming engine
    let streaming_config = ears::streaming::StreamingConfig::default();
    let vad_config = ears::vad::VadConfig {
        sample_rate: 16000,
        speech_threshold: config.vad.speech_threshold,
        min_speech_duration_ms: config.vad.min_speech_duration_ms,
        max_silence_duration_ms: config.vad.max_silence_duration_ms,
        pre_speech_buffer_ms: config.vad.pre_speech_buffer_ms,
    };
    let typing_config = ears::progressive_typing::ProgressiveTypingConfig::default();
    let temp_dir = config.state_dir.clone();
    let mut engine = ears::streaming_engine::StreamingEngine::new(
        whisper_client,
        streaming_config,
        vad_config,
        typing_config,
        temp_dir,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create streaming engine: {}", e))?;

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ears::streaming_engine::StreamingEvent>();
    engine.set_event_sender(event_tx.clone());

    // Disable typing for ws-listen — only emit IPC events, never type into windows
    {
        let (_settings_tx, mut settings_rx) =
            tokio::sync::watch::channel(ears::tui::TypingSettings {
                progressive_typing: false,
                auto_correction: false,
                typing_mode: config.typing_mode,
                auto_enter: false,
                text_filters: config.text_filters.clone(),
                language: config.language.clone(),
            });
        // Apply initial settings
        let s = settings_rx.borrow_and_update().clone();
        engine.set_typing_enabled(
            s.progressive_typing,
            s.auto_correction,
            s.typing_mode,
            s.auto_enter,
        );
        engine.set_text_filters(s.text_filters, s.language);

        // Shutdown channel
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        // Audio processing task
        let pipeline_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    audio = audio_rx.recv() => {
                        match audio {
                            Some(samples) => {
                                if let Err(e) = engine.process_audio(&samples).await {
                                    tracing::warn!("Audio processing error: {}", e);
                                }
                            }
                            None => {
                                tracing::debug!("Audio channel closed");
                                break;
                            }
                        }
                    }
                    _ = settings_rx.changed() => {
                        let s = settings_rx.borrow_and_update().clone();
                        engine.set_typing_enabled(s.progressive_typing, s.auto_correction, s.typing_mode, s.auto_enter);
                        engine.set_text_filters(s.text_filters, s.language);
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::debug!("WS VAD pipeline shutdown requested");
                        break;
                    }
                }
            }
        });

        // IPC server on custom socket path (avoids conflict with desktop ears)
        ears::ipc::start_ipc_server_at(socket_path.clone(), ipc_rx);

        let ws_save_to_clipboard = config.save_to_clipboard;
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = ipc_tx.send(event.clone());
                match event {
                    ears::streaming_engine::StreamingEvent::SegmentCompleted {
                        text,
                        duration_ms,
                    } => {
                        tracing::info!("Segment: \"{}\" ({}ms)", text, duration_ms);
                        if ws_save_to_clipboard && !text.is_empty() {
                            TextInput::copy_to_clipboard(&text);
                        }
                    }
                    ears::streaming_engine::StreamingEvent::Error(msg) => {
                        tracing::warn!("Streaming error: {}", msg);
                    }
                    _ => {}
                }
            }
        });

        eprintln!("WebSocket VAD listening on ws://{}:{}", host, port);

        // Wait for signal
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }

        let _ = shutdown_tx.send(true);
        let _ = pipeline_handle.await;
        ws_handle.abort();
    }

    ears::ipc::cleanup_socket_at(&socket_path);

    Ok(())
}

/// Main toggle logic: start recording or stop and transcribe
async fn handle_toggle(config: &Config) -> Result<()> {
    // Serialize toggle operations across processes to prevent races on the
    // state file and audio file when the keybind is pressed rapidly.
    let lock_path = config.state_dir.join("toggle.lock");
    let mut lock = FileLock::new(&lock_path).context("Failed to create toggle lock")?;
    if !lock.try_lock().context("Failed to check toggle lock")? {
        tracing::info!("Toggle already in progress, ignoring");
        return Ok(());
    }

    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;

    state_mgr.load_state().context("Failed to load state")?;

    let pid_file = config.state_dir.join("recording.pid");
    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(120));

    if let Err(e) = process_mgr.cleanup_stale() {
        tracing::debug!("Stale process cleanup: {}", e);
    }

    // If VAD is active, toggle should not interfere
    if state_mgr.current_state() == StateEnum::VadActive {
        tracing::info!("VAD is active, toggle ignored");
        return Ok(());
    }

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
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());
    let client = WhisperClient::new(server_url.to_string())
        .with_language(language)
        .with_api_key(config.api_key.clone())
        .with_model(model)
        .with_prompt(config.prompt.clone());
    if client.health_check().await.is_err() {
        tracing::error!("Whisper server health check failed");
        AudioFeedback::beep_error().ok();
        Notifications::error("Whisper server not running!").ok();
        anyhow::bail!("Whisper server not available");
    }

    tracing::info!("Health check passed in {:?}", health_start.elapsed());

    let audio_file = config.state_dir.join("recording.wav");
    if audio_file.exists() {
        if let Err(e) = std::fs::remove_file(&audio_file) {
            tracing::warn!("Failed to remove old recording file: {}", e);
        }
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
        if let Err(e) = process_mgr.cleanup_stale() {
            tracing::debug!("Stale process cleanup: {}", e);
        }
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

    if metadata.len() <= ears::WAV_HEADER_SIZE {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording too short").ok();
        std::fs::remove_file(&audio_file).ok();
        tracing::error!(
            "Audio file has no audio data ({} bytes, header is {})",
            metadata.len(),
            ears::WAV_HEADER_SIZE
        );
        anyhow::bail!("Recording file contains no audio data");
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

    // Guard ensures state resets to Idle even if we return early via `?` or panic.
    // This prevents getting stuck in Transcribing after crashes or unexpected errors.
    let _state_guard = ears::state::StateResetGuard::new(&config.state_dir);

    let lang_start = std::time::Instant::now();
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    tracing::info!("Language detection took {:?}", lang_start.elapsed());
    if let Some(ref lang) = language {
        tracing::info!("Using language: {} (from keyboard layout)", lang);
    }

    let transcribe_start = std::time::Instant::now();
    let (server_url, model) = config.resolve_server(language.as_deref());
    let client = WhisperClient::new(server_url.to_string())
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(model)
        .with_prompt(config.prompt.clone());
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

            // Copy to clipboard if enabled
            if config.save_to_clipboard && !filtered_text.is_empty() {
                TextInput::copy_to_clipboard(&filtered_text);
            }

            run_post_transcribe_hook(&audio_file, &filtered_text);
        }
        Ok(_) => {
            AudioFeedback::beep_error().ok();
            Notifications::info("No speech detected").ok();
            tracing::info!("No speech detected");
        }
        Err(e) => {
            AudioFeedback::beep_error().ok();
            Notifications::error(&format!("Transcription failed: {}", e)).ok();
            tracing::error!("Transcription failed: {}", e);

            std::fs::remove_file(&audio_file).ok();

            // Guard will reset state to Idle on drop
            return Err(e.into());
        }
    }

    std::fs::remove_file(&audio_file).ok();

    // Explicit transition (guard also resets on drop, but this is the clean path)
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
    fn test_mask_secret() {
        // Long secret keeps a prefix/suffix, hides the middle and never leaks it.
        let masked = mask_secret("gsk_abcdefghijklmnopqrstuvwxyz0123");
        assert_eq!(masked, "set (gsk_…123)");
        assert!(!masked.contains("abcdef"));

        // Short secrets reveal nothing.
        assert_eq!(mask_secret("short"), "set (hidden)");
        assert_eq!(mask_secret("12345678"), "set (hidden)");
    }

    #[test]
    fn test_invalid_scheme() {
        let url = Url::parse("ftp://localhost:8080").unwrap();
        assert!(!matches!(url.scheme(), "http" | "https"));
    }
}
