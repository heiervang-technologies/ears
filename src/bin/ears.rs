//! ears - Speech recognition daemon CLI
//!
//! Main binary for the ears daemon

use anyhow::Result;
use clap::Parser;
use ears::{Config, State};

#[derive(Parser)]
#[command(name = "ears")]
#[command(about = "Speech recognition daemon", long_about = None)]
struct Cli {
    /// Launch interactive TUI mode
    #[arg(short = 't', long = "tui")]
    tui: bool,

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

fn main() -> Result<()> {
    // Initialize tracing/logging suppression for tests
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
        )
        .try_init();

    let cli = Cli::parse();

    // Handle flags in order of precedence
    if cli.tui {
        return ears::tui::run();
    }

    if cli.select {
        println!("Device selection not yet implemented");
        return Ok(());
    }

    if cli.list {
        println!("Device listing not yet implemented");
        return Ok(());
    }

    if cli.current {
        let device = Config::load_device()?;
        println!("Current device: {}", device);
        let config_file = Config::config_dir()?.join("device");
        if config_file.exists() {
            println!("Config file: {}", config_file.display());
        } else {
            println!("(using default)");
        }
        return Ok(());
    }

    if let Some(url) = cli.server {
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
            Config::save_server(&url)?;
            println!("Server set to: {}", url);
        }
        return Ok(());
    }

    // Main toggle logic (no flags provided)
    let state = State::new()?;
    state.cleanup_stale()?;

    if state.is_recording() {
        println!("Recording active - would stop and transcribe");
    } else {
        println!("No recording - would start recording");
    }

    Ok(())
}
