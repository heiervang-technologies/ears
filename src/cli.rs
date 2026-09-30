use clap::{Parser, Subcommand};

/// A production-grade speech recognition daemon for Linux
#[derive(Parser, Debug)]
#[command(name = "ears")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Configuration profile (loads config.{profile}.toml instead of config.toml)
    #[arg(long, short = 'p', global = true)]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Toggle recording/transcription (for keyboard shortcuts)
    #[command(alias = "t")]
    Toggle {
        /// Show the transcript as inline ghost text while recording (via the
        /// fcitx5 `earsghost` addon) and commit it on the second press.
        #[arg(long)]
        ghost: bool,
    },

    /// Toggle VAD (Voice Activity Detection) mode (for keyboard shortcuts)
    #[command(alias = "v")]
    Vad,

    /// Toggle VAD with ghost completion: while you speak, the transcript is
    /// shown as inline preedit ("ghost text") in the focused app via the
    /// fcitx5 `earsghost` addon, and committed when the utterance ends.
    /// Shares the VAD toggle: running either command again stops listening.
    #[command(alias = "g")]
    Ghost,

    /// Show or set how ghost text looks (colour, underline), and write it
    /// into the apps that support it (Alacritty, Hover).
    GhostStyle {
        /// `#rrggbb`, `#rgb`, a preset (grey, blue-grey, orange, yellow,
        /// green-yellow), or `default` to leave colour to each app
        color: Option<String>,
        /// Underline the ghost text
        #[arg(long, conflicts_with = "no_underline")]
        underline: bool,
        /// Do not underline the ghost text
        #[arg(long)]
        no_underline: bool,
        /// Colour of the settled start of the ghost, the part that will no
        /// longer change (same formats; `default` draws it like the rest)
        #[arg(long, value_name = "COLOR")]
        frozen: Option<String>,
    },

    /// Internal: ghost preview loop for a push-to-talk recording
    #[command(hide = true)]
    GhostPreview {
        /// PID of the recorder this preview belongs to
        #[arg(long)]
        recorder_pid: i32,
        /// Start time (clock ticks since boot) of that recorder, so a
        /// recycled PID is not mistaken for it
        #[arg(long)]
        recorder_start: u64,
    },

    /// Start WebSocket server for remote audio input (VAD pipeline)
    #[command(alias = "ws")]
    WsListen {
        /// Host to bind to
        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// Port to listen on
        #[arg(long, default_value_t = 8765)]
        port: u16,

        /// Custom IPC socket path (avoids conflicting with desktop ears instance)
        #[arg(long)]
        socket: Option<String>,
    },

    /// Audio device management
    #[command(alias = "d")]
    Device {
        #[command(subcommand)]
        action: Option<DeviceAction>,
    },

    /// Show or set the default profile
    Profile {
        /// Profile name to activate (omit to show current, "default" to clear)
        name: Option<String>,
    },

    /// Configure whisper server
    Server {
        /// Set server URL (if omitted, shows current server)
        url: Option<String>,
    },

    /// Validate the active config: print a summary and check server health
    Test {
        /// Optional audio file (WAV) to run a sample transcription against
        file: Option<String>,
    },

    /// Toggle auto-enter on the running ears instance
    #[command(alias = "ae")]
    AutoEnter,

    /// Type transcripts into the focused window: on, off, toggle or status.
    /// Off keeps transcribing and publishing on the IPC socket (for
    /// `talking-stick listen`) without typing. Persists across restarts.
    Typing {
        /// on | off | toggle | status (default: status)
        #[arg(default_value = "status", value_parser = ["on", "off", "toggle", "status"])]
        action: String,
    },

    /// Select audio device with fzf (shortcut for `device select`)
    #[command(alias = "s", hide = true)]
    Select,

    /// List available audio devices (shortcut for `device list`)
    #[command(alias = "l", hide = true)]
    List,

    /// Show current device (shortcut for `device current`)
    #[command(alias = "c", hide = true)]
    Current,
}

#[derive(Subcommand, Debug)]
pub enum DeviceAction {
    /// List available audio devices
    #[command(alias = "l")]
    List,

    /// Select audio device with fzf
    #[command(alias = "s")]
    Select,

    /// Show current device
    #[command(alias = "c")]
    Current,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_no_args() {
        let cli = Cli::try_parse_from(["ears"]).unwrap();
        assert!(cli.command.is_none()); // Should launch TUI
        assert!(cli.profile.is_none());
    }

    #[test]
    fn test_cli_toggle() {
        let cli = Cli::try_parse_from(["ears", "toggle"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Toggle { .. })));
    }

    #[test]
    fn test_cli_toggle_alias() {
        let cli = Cli::try_parse_from(["ears", "t"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Toggle { .. })));
    }

    #[test]
    fn test_cli_device_list() {
        let cli = Cli::try_parse_from(["ears", "device", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Device {
                action: Some(DeviceAction::List)
            })
        ));
    }

    #[test]
    fn test_cli_device_select() {
        let cli = Cli::try_parse_from(["ears", "device", "select"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Device {
                action: Some(DeviceAction::Select)
            })
        ));
    }

    #[test]
    fn test_cli_device_current() {
        let cli = Cli::try_parse_from(["ears", "device", "current"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Device {
                action: Some(DeviceAction::Current)
            })
        ));
    }

    #[test]
    fn test_cli_device_no_action() {
        let cli = Cli::try_parse_from(["ears", "device"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Device { action: None })
        ));
    }

    #[test]
    fn test_cli_select_compat() {
        let cli = Cli::try_parse_from(["ears", "select"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Select)));
    }

    #[test]
    fn test_cli_list_compat() {
        let cli = Cli::try_parse_from(["ears", "list"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::List)));
    }

    #[test]
    fn test_cli_current_compat() {
        let cli = Cli::try_parse_from(["ears", "current"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Current)));
    }

    #[test]
    fn test_cli_profile_show() {
        let cli = Cli::try_parse_from(["ears", "profile"]).unwrap();
        match cli.command {
            Some(Commands::Profile { name }) => assert!(name.is_none()),
            _ => panic!("Expected Profile command"),
        }
    }

    #[test]
    fn test_cli_profile_set() {
        let cli = Cli::try_parse_from(["ears", "profile", "qwen3-asr"]).unwrap();
        match cli.command {
            Some(Commands::Profile { name }) => {
                assert_eq!(name.as_deref(), Some("qwen3-asr"));
            }
            _ => panic!("Expected Profile command"),
        }
    }

    #[test]
    fn test_cli_server_show() {
        let cli = Cli::try_parse_from(["ears", "server"]).unwrap();
        match cli.command {
            Some(Commands::Server { url }) => assert!(url.is_none()),
            _ => panic!("Expected Server command"),
        }
    }

    #[test]
    fn test_cli_server_set() {
        let cli = Cli::try_parse_from(["ears", "server", "http://localhost:8080"]).unwrap();
        match cli.command {
            Some(Commands::Server { url }) => {
                assert_eq!(url.as_deref(), Some("http://localhost:8080"));
            }
            _ => panic!("Expected Server command"),
        }
    }

    #[test]
    fn test_cli_test_no_file() {
        let cli = Cli::try_parse_from(["ears", "test"]).unwrap();
        match cli.command {
            Some(Commands::Test { file }) => assert!(file.is_none()),
            _ => panic!("Expected Test command"),
        }
    }

    #[test]
    fn test_cli_test_with_file() {
        let cli = Cli::try_parse_from(["ears", "test", "sample.wav"]).unwrap();
        match cli.command {
            Some(Commands::Test { file }) => assert_eq!(file.as_deref(), Some("sample.wav")),
            _ => panic!("Expected Test command"),
        }
    }

    #[test]
    fn test_cli_profile() {
        let cli = Cli::try_parse_from(["ears", "--profile", "groq", "toggle"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("groq"));
        assert!(matches!(cli.command, Some(Commands::Toggle { .. })));
    }

    #[test]
    fn test_cli_profile_short() {
        let cli = Cli::try_parse_from(["ears", "-p", "groq"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("groq"));
    }
}
