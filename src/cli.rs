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
    Toggle,

    /// Toggle VAD (Voice Activity Detection) mode (for keyboard shortcuts)
    #[command(alias = "v")]
    Vad,

    /// Select audio device with fzf
    #[command(alias = "s")]
    Select,

    /// List available audio devices
    #[command(alias = "l")]
    List,

    /// Show current device
    #[command(alias = "c")]
    Current,

    /// Configure whisper server
    Server {
        /// Set server URL (if omitted, shows current server)
        url: Option<String>,
    },
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
        assert!(matches!(cli.command, Some(Commands::Toggle)));
    }

    #[test]
    fn test_cli_toggle_alias() {
        let cli = Cli::try_parse_from(["ears", "t"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Toggle)));
    }

    #[test]
    fn test_cli_select() {
        let cli = Cli::try_parse_from(["ears", "select"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Select)));
    }

    #[test]
    fn test_cli_select_alias() {
        let cli = Cli::try_parse_from(["ears", "s"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Select)));
    }

    #[test]
    fn test_cli_list() {
        let cli = Cli::try_parse_from(["ears", "list"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::List)));
    }

    #[test]
    fn test_cli_list_alias() {
        let cli = Cli::try_parse_from(["ears", "l"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::List)));
    }

    #[test]
    fn test_cli_current() {
        let cli = Cli::try_parse_from(["ears", "current"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Current)));
    }

    #[test]
    fn test_cli_current_alias() {
        let cli = Cli::try_parse_from(["ears", "c"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Current)));
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
    fn test_cli_profile() {
        let cli = Cli::try_parse_from(["ears", "--profile", "groq", "toggle"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("groq"));
        assert!(matches!(cli.command, Some(Commands::Toggle)));
    }

    #[test]
    fn test_cli_profile_short() {
        let cli = Cli::try_parse_from(["ears", "-p", "groq"]).unwrap();
        assert_eq!(cli.profile.as_deref(), Some("groq"));
    }
}
