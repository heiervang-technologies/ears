//! TUI (Terminal User Interface) module
//!
//! Provides an interactive terminal interface for ears with vim-style navigation.

mod app;
mod event;
pub mod ui; // Make ui module public for testing

pub use app::{App, ClickAction, ClickableRegion, EditableField, Panel};
pub use event::{Event, EventHandler};

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use crate::config::Config;
use crate::continuous_capture::{ContinuousCapture, ContinuousCaptureConfig};
use crate::progressive_typing::ProgressiveTypingConfig;
use crate::state::{State as EarsState, StateManager};
use crate::streaming::StreamingConfig;
use crate::streaming_engine::{StreamingEngine, StreamingEvent};
use crate::vad::VadConfig;
use crate::whisper::WhisperClient;
use crate::KeyboardLayout;

/// Initialize the terminal for TUI mode
pub fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state
pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

/// Start the VAD audio processing pipeline.
///
/// Returns the shutdown sender and a join handle for the processing task.
async fn start_vad_pipeline(
    config: &Config,
    event_tx: mpsc::UnboundedSender<StreamingEvent>,
) -> Result<(watch::Sender<bool>, tokio::task::JoinHandle<()>)> {
    // Create whisper client with language from config/keyboard layout
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let whisper_client =
        Arc::new(WhisperClient::new(config.whisper_server.to_string()).with_language(language));

    // Health check
    whisper_client
        .health_check()
        .await
        .map_err(|e| anyhow::anyhow!("Whisper server health check failed: {}", e))?;

    // Audio channel
    let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Vec<f32>>();

    // Start continuous capture
    let capture_config = ContinuousCaptureConfig {
        device: config.device.clone(),
        ..ContinuousCaptureConfig::default()
    };
    let temp_dir = config.state_dir.clone();
    let mut capture = ContinuousCapture::new(capture_config, temp_dir.clone());
    capture.set_audio_sender(audio_tx);
    capture.start().await?;

    // Create streaming engine
    let streaming_config = StreamingConfig::default();
    let vad_config = VadConfig::default();
    let typing_config = ProgressiveTypingConfig::default();
    let mut engine = StreamingEngine::new(
        whisper_client,
        streaming_config,
        vad_config,
        typing_config,
        temp_dir,
    );
    engine.set_event_sender(event_tx);

    // Shutdown channel
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    // Spawn audio processing task
    let handle = tokio::spawn(async move {
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
                _ = shutdown_rx.changed() => {
                    tracing::debug!("VAD pipeline shutdown requested");
                    break;
                }
            }
        }
        // Capture is dropped here, which calls stop() via Drop
        drop(capture);
    });

    Ok((shutdown_tx, handle))
}

/// Run the TUI application
pub async fn run() -> Result<()> {
    let mut terminal = init_terminal()?;
    let mut app = App::new();
    let event_handler = EventHandler::new(250);

    // Load config and create state manager for waybar integration
    let config = Config::load().unwrap_or_default();
    let mut state_mgr = StateManager::new(&config.state_dir)?;

    // Streaming event channel
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<StreamingEvent>();

    // VAD pipeline state
    let mut vad_running = false;
    let mut vad_shutdown: Option<watch::Sender<bool>> = None;
    let mut vad_handle: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        // Clear clickable regions before rendering
        app.clear_clickable_regions();
        terminal.draw(|f| ui::render(&mut app, f))?;

        match event_handler.next()? {
            Event::Key(key) => {
                if !app.handle_key(key)? {
                    break;
                }
            }
            Event::Mouse(mouse) => {
                if !app.handle_mouse(mouse)? {
                    break;
                }
            }
            Event::Tick => {
                app.handle_tick();
            }
            Event::Resize(_, _) => {
                // Terminal resize is handled automatically by ratatui
                // on the next draw() call, no action needed
            }
        }

        // Drain streaming events
        while let Ok(event) = event_rx.try_recv() {
            app.handle_streaming_event(event);
        }

        // Check if VAD state changed
        if app.vad_active && !vad_running {
            // Start VAD pipeline
            match start_vad_pipeline(&config, event_tx.clone()).await {
                Ok((shutdown, handle)) => {
                    vad_shutdown = Some(shutdown);
                    vad_handle = Some(handle);
                    vad_running = true;
                    if let Err(e) = state_mgr.transition(EarsState::VadActive) {
                        tracing::warn!("State transition error: {}", e);
                    }
                    app.add_log("VAD pipeline started");
                }
                Err(e) => {
                    app.vad_active = false;
                    app.add_log(&format!("Failed to start VAD: {}", e));
                }
            }
        } else if !app.vad_active && vad_running {
            // Stop VAD pipeline
            if let Some(tx) = vad_shutdown.take() {
                let _ = tx.send(true);
            }
            if let Some(handle) = vad_handle.take() {
                let _ = handle.await;
            }
            vad_running = false;
            app.is_speaking = false;
            if let Err(e) = state_mgr.transition(EarsState::Idle) {
                tracing::warn!("State transition error: {}", e);
            }
            app.add_log("VAD pipeline stopped");
        }
    }

    // Clean up VAD pipeline on exit
    if vad_running {
        if let Some(tx) = vad_shutdown.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = vad_handle.take() {
            let _ = handle.await;
        }
        let _ = state_mgr.transition(EarsState::Idle);
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}
