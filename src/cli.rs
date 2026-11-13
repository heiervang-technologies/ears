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

/// What `ears vad` does; without one it toggles.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadAction {
    Start,
    Stop,
    Status,
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

    /// Toggle VAD (Voice Activity Detection) mode (for keyboard shortcuts).
    /// Scripts should say what they mean: `start` never stops a running VAD,
    /// `stop` never starts one, `status` exits 0 while it runs.
    #[command(alias = "v")]
    Vad {
        #[arg(value_enum)]
        action: Option<VadAction>,
    },

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

    /// Watch the live frozen/revisable ghost boundary without changing focus
    GhostWatch {
        /// Emit changed snapshots as JSON lines for external visualizers
        #[arg(long)]
        json: bool,
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

        /// Keep each transcribed utterance as a 16 kHz mono WAV in this
        /// directory and report it as `audio_path` on `SegmentCompleted`
        /// (newest 32 kept; the consumer may delete them)
        #[arg(long, value_name = "DIR")]
        keep_audio: Option<std::path::PathBuf>,

        /// Publish the utterance in progress as `Partial { text, settled }`
        /// events while the speaker is still talking (the same live decoding
        /// the desktop ghost uses; `live_decoding = "continuous"` streams it),
        /// so a remote client can draw words as they are heard
        #[arg(long)]
        live: bool,

        /// Drop agents' `say` lines this mic picks up (`[say_echo]` in the
        /// config sets the log and window; off for ws-listen without this)
        #[arg(long)]
        say_echo: bool,
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

    /// Report whether desktop VAD is working, from its health snapshot
    ///
    /// Reads $XDG_RUNTIME_DIR/ears/vad-health.json (published once per second
    /// by `ears vad`) and checks it against the running process: is the
    /// process that wrote it alive, is the snapshot fresh, is the pipeline
    /// making progress, and is the microphone or the server worth a look.
    ///
    /// Exit status:
    ///   0  healthy
    ///   1  not running (never started, stopped, or the process is gone)
    ///   2  stale or stuck (the process is alive but stopped reporting, or
    ///      reports no audio, a stalled stage or an unreachable server)
    ///   3  running with warnings (microphone quality, degraded server)
    ///   4  the snapshot cannot be read
    #[command(verbatim_doc_comment)]
    Health {
        /// Print the verdict and the snapshot as JSON
        #[arg(long)]
        json: bool,
    },

    /// Run the Silero VAD over a 16 kHz 16-bit WAV and print where speech is:
    /// no microphone, no server, no sound. For checking a build on a new machine.
    VadFile {
        /// The WAV to scan
        file: std::path::PathBuf,
        /// Speech probability that counts as speech
        #[arg(long, default_value_t = 0.5)]
        threshold: f32,
    },

    /// Validate the active config: print a summary and check server health
    Test {
        /// Optional audio file (WAV) to run a sample transcription against
        file: Option<String>,
    },

    /// Measure the microphone: background noise, speech level, clipping,
    /// narrow band (Bluetooth call audio), and how well a read sentence
    /// transcribes
    #[command(name = "mic-check")]
    MicCheck {
        /// Device to check (default: the configured one)
        #[arg(long)]
        device: Option<String>,
        /// Only measure; don't transcribe the sentence
        #[arg(long)]
        no_transcribe: bool,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Print a short marker for shell prompts (starship) while recording,
    /// transcribing or in VAD mode; prints nothing while idle
    Prompt {
        /// Only print when in this state (one prompt segment per state)
        #[arg(long, value_parser = ["recording", "transcribing", "vad"])]
        state: Option<String>,
        /// Text while recording
        #[arg(long, default_value = "● rec")]
        recording: String,
        /// Text while transcribing
        #[arg(long, default_value = "◌ …")]
        transcribing: String,
        /// Text while VAD mode is on
        #[arg(long, default_value = "◉ vad")]
        vad: String,
        /// Text while VAD is in standby (listening for the wake phrase)
        #[arg(long, default_value = "○ vad")]
        standby: String,
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

    /// Standby: on (only the wake phrase is acted on), off (resume), toggle
    /// or status. The same switch as the spoken wake and standby phrases, for
    /// tools, e.g. a voice call that needs the words.
    Standby {
        /// on | off | toggle | status (default: status)
        #[arg(default_value = "status", value_parser = ["on", "off", "toggle", "status"])]
        action: String,
    },

    /// Command mode: on, off, toggle or status. The same switch as saying
    /// "command", for tools, e.g. a voice call that needs the words.
    #[command(name = "command")]
    CommandMode {
        /// on | off | toggle | status (default: status)
        #[arg(default_value = "status", value_parser = ["on", "off", "toggle", "status"])]
        action: String,
    },

    /// Switch the VAD mode by its colour, as saying the colour does: orange
    /// (standby), blue (dictation), yellow (talking stick), purple (command
    /// mode), pink (vision). `toggle` goes between orange and blue: a key
    /// for "stop typing / type again".
    Colour {
        /// toggle | orange | blue | yellow | purple | pink
        #[arg(value_parser = ["toggle", "orange", "blue", "yellow", "purple", "pink"])]
        colour: String,
    },

    /// Command mode: teach an action your phrasing. The next utterance in
    /// command mode becomes one of its anchors (it is not fired). Without
    /// ACTION, list the actions and how many anchors each has.
    Teach {
        /// Action name from ~/.config/ears/actions.toml or a built-in.
        action: Option<String>,
    },

    /// Ask Markus a question with discrete options, out loud: ears VAD turns
    /// red, shows them above its line, and the option he reads out (or its
    /// number) is printed. Heard by its words and, when those are unclear or
    /// ASR is down, by embeddinggemma2. Exit 0 chosen, 1 cancelled ("cancel", "none of
    /// these"), 2 timed out, 3 VAD not running.
    Decide {
        /// The question, shown above the options.
        question: String,
        /// The options (2 to 6), read out as they are written.
        #[arg(required = true, num_args = 2..=6)]
        options: Vec<String>,
        /// Give up after this many seconds, a quick time event: the overlay
        /// drains a bar meanwhile. Ignored when config `decide_timeouts` is
        /// false. Default: wait.
        #[arg(long)]
        timeout: Option<u64>,
        /// Print the option's index (0-based) instead of its text ("other"
        /// for an answer of your own).
        #[arg(long)]
        index: bool,
        /// Leave room for an answer of your own: what is not an option is
        /// printed as said.
        #[arg(long)]
        other: bool,
    },

    /// Find something on screen with SAM 3 and act on it (vision mode).
    /// "the send button" outlines the best match in pink; then say
    /// "click", "right click", "middle click", "next", a number, "up
    /// twenty" / "down five" (scroll there), or describe something else.
    /// A verb up front ("right click the blue folder") picks the action.
    /// Exit 0 done, 1 cancelled or nothing found, 2 no answer in time.
    Look {
        /// What to find, e.g. "right click the second send button".
        #[arg(required = true, num_args = 1..)]
        words: Vec<String>,
        /// Act on the best match without asking (needs a verb).
        #[arg(long)]
        yes: bool,
        /// Print the matches as JSON and exit, without acting.
        #[arg(long)]
        json: bool,
    },

    /// Live subtitles of what VAD hears, drawn above the ears LED line
    /// (for a stream). Without on|off, say whether they are on.
    Subtitles {
        /// on or off
        state: Option<String>,
    },

    /// Compensating Scaffolding: known mishearings of command phrases for the
    /// ASR model in use, kept in cs files (docs/rfc/0002-compensating-scaffolding.md)
    Cs {
        #[command(subcommand)]
        action: CsAction,
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

/// `ears cs` subcommands. Each is dispatched in `main.rs` (`handle_cs`).
#[derive(Subcommand, Debug)]
pub enum CsAction {
    /// List the entries that apply to the ASR model (file, target, meant,
    /// heard, mode, source, risk, status), marking flagged ones. Exit 1 when
    /// an active entry would now be rejected.
    Check {
        /// ASR model id to check for (default: as VAD resolves it, the
        /// configured model or the first the server lists)
        #[arg(long)]
        model: Option<String>,
        /// Also check this cs file, as if it were installed last in cs.d
        /// (for reviewing one, e.g. from `ears cs migrate`)
        #[arg(long, value_name = "FILE")]
        file: Vec<std::path::PathBuf>,
    },

    /// Turn the hand-written variants in config.toml (and actions.toml) into
    /// a reviewable cs file. Neither file is changed.
    Migrate {
        /// Write the cs file here instead of to stdout
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
        /// Config to read (default: the active profile's)
        #[arg(long, value_name = "FILE")]
        config: Option<std::path::PathBuf>,
        /// Command-mode actions to read (default: actions.toml beside it)
        #[arg(long, value_name = "FILE")]
        actions: Option<std::path::PathBuf>,
        /// `meta.model` of the file: "any", or the ASR model the variants
        /// were collected with
        #[arg(long, default_value = "any")]
        model: String,
    },
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
    fn test_cli_cs() {
        let cli = Cli::try_parse_from(["ears", "cs", "check", "--model", "m", "--file", "a.toml"])
            .unwrap();
        match cli.command {
            Some(Commands::Cs {
                action: CsAction::Check { model, file },
            }) => {
                assert_eq!(model.as_deref(), Some("m"));
                assert_eq!(file, vec![std::path::PathBuf::from("a.toml")]);
            }
            other => panic!("{other:?}"),
        }
        let cli = Cli::try_parse_from(["ears", "cs", "migrate"]).unwrap();
        match cli.command {
            Some(Commands::Cs {
                action: CsAction::Migrate { out, model, .. },
            }) => {
                assert!(out.is_none());
                assert_eq!(model, "any");
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["ears", "cs"]).is_err());
    }

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
    fn test_cli_health() {
        let cli = Cli::try_parse_from(["ears", "health"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Health { json: false })
        ));
        let cli = Cli::try_parse_from(["ears", "health", "--json"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Health { json: true })));
        assert!(Cli::try_parse_from(["ears", "health", "extra"]).is_err());
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
