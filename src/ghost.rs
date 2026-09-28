//! Ghost completion: show in-progress speech as inline preedit text.
//!
//! Ghost mode does not type anything while you speak. Partial transcripts
//! are sent to the `earsghost` fcitx5 addon (see `fcitx5-addon/`), which
//! shows them as *preedit* in whichever application has input focus: the
//! app draws the text at its cursor, usually underlined, without it being
//! part of the document or reaching the shell. When the utterance ends the
//! final transcript is *committed*, which is when the application actually
//! receives it.
//!
//! Wire protocol (one line per command, one reply line per command):
//!
//! ```text
//! P <text>   show <text> as the ghost (replaces the previous one)
//! C <text>   clear the ghost and commit <text>
//! X          clear the ghost
//! S          status
//! reply:     "OK preedit" | "OK panel" | "OK none" | "ERR <reason>"
//! ```
//!
//! Text escaping: backslash becomes `\\`, newline becomes `\n`.
//!
//! The addon clears any visible ghost when the connection closes, so a
//! crashed or stopped ears never leaves stale text behind.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Per-command I/O deadline. The addon answers from fcitx5's event loop in
/// well under a millisecond; anything slower means fcitx5 is wedged.
const IO_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Error)]
pub enum GhostError {
    #[error("ghost addon not reachable at {path}: {source}")]
    Unavailable {
        path: String,
        source: std::io::Error,
    },
    #[error("ghost addon I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("ghost addon replied with an error: {0}")]
    Rejected(String),
}

/// How the addon displayed or delivered the last command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostDisplay {
    /// Drawn inline by the focused application.
    Preedit,
    /// The application cannot draw preedit; fcitx5 shows it in its panel.
    Panel,
    /// No input context has focus; nothing was shown or delivered.
    None,
}

/// Default socket path, shared with the addon: `$XDG_RUNTIME_DIR/ears/ghost.sock`.
pub fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    runtime.join("ears").join("ghost.sock")
}

/// Escape text for one protocol line.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Client for the `earsghost` fcitx5 addon.
///
/// Connects lazily and reconnects after any failure, so fcitx5 restarts are
/// tolerated. Dropping the client closes the connection, which makes the
/// addon clear any ghost still on screen.
pub struct GhostClient {
    path: PathBuf,
    conn: Option<(UnixStream, BufReader<UnixStream>)>,
}

impl GhostClient {
    pub fn new(path: PathBuf) -> Self {
        Self { path, conn: None }
    }

    /// Is the addon reachable right now?
    pub fn probe(&mut self) -> Result<GhostDisplay, GhostError> {
        self.send("S")
    }

    /// Show `text` as the ghost, replacing the previous one.
    pub fn preedit(&mut self, text: &str) -> Result<GhostDisplay, GhostError> {
        self.send(&format!("P {}", escape(text)))
    }

    /// Clear the ghost and deliver `text` to the focused application.
    pub fn commit(&mut self, text: &str) -> Result<GhostDisplay, GhostError> {
        self.send(&format!("C {}", escape(text)))
    }

    /// Clear the ghost without delivering anything.
    pub fn clear(&mut self) -> Result<GhostDisplay, GhostError> {
        self.send("X")
    }

    fn connect(&mut self) -> Result<(), GhostError> {
        if self.conn.is_some() {
            return Ok(());
        }
        let stream = UnixStream::connect(&self.path).map_err(|source| GhostError::Unavailable {
            path: self.path.display().to_string(),
            source,
        })?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let reader = BufReader::new(stream.try_clone()?);
        self.conn = Some((stream, reader));
        Ok(())
    }

    fn send(&mut self, line: &str) -> Result<GhostDisplay, GhostError> {
        self.connect()?;
        let result = self.exchange(line);
        if result.is_err() {
            // Any I/O failure invalidates the stream; reconnect next time.
            self.conn = None;
        }
        result
    }

    fn exchange(&mut self, line: &str) -> Result<GhostDisplay, GhostError> {
        let (stream, reader) = self.conn.as_mut().expect("connected above");
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut reply = String::new();
        if reader.read_line(&mut reply)? == 0 {
            return Err(GhostError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "addon closed the connection",
            )));
        }
        parse_reply(reply.trim_end())
    }
}

fn parse_reply(reply: &str) -> Result<GhostDisplay, GhostError> {
    let mut words = reply.split_whitespace();
    match (words.next(), words.next()) {
        (Some("OK"), Some("preedit")) => Ok(GhostDisplay::Preedit),
        (Some("OK"), Some("panel")) => Ok(GhostDisplay::Panel),
        (Some("OK"), _) => Ok(GhostDisplay::None),
        _ => Err(GhostError::Rejected(reply.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    /// Minimal stand-in for the addon: records lines, answers "OK preedit".
    fn fake_addon(path: PathBuf) -> mpsc::Receiver<String> {
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut writer = stream.try_clone().unwrap();
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let _ = tx.send(line);
                    let _ = writer.write_all(b"OK preedit\n");
                }
            }
        });
        rx
    }

    #[test]
    fn escape_roundtrips_specials() {
        assert_eq!(escape("a\\b\nc\r"), "a\\\\b\\nc");
        assert_eq!(escape("plain text"), "plain text");
    }

    #[test]
    fn parse_replies() {
        assert_eq!(parse_reply("OK preedit").unwrap(), GhostDisplay::Preedit);
        assert_eq!(parse_reply("OK panel").unwrap(), GhostDisplay::Panel);
        assert_eq!(parse_reply("OK none").unwrap(), GhostDisplay::None);
        assert_eq!(
            parse_reply("OK preedit Alacritty").unwrap(),
            GhostDisplay::Preedit
        );
        assert!(parse_reply("ERR nope").is_err());
        assert!(parse_reply("").is_err());
    }

    #[test]
    fn client_speaks_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = GhostClient::new(path);

        assert_eq!(client.preedit("hello wor").unwrap(), GhostDisplay::Preedit);
        assert_eq!(
            client.commit("hello world\nls").unwrap(),
            GhostDisplay::Preedit
        );
        client.clear().unwrap();

        assert_eq!(rx.recv().unwrap(), "P hello wor");
        assert_eq!(rx.recv().unwrap(), "C hello world\\nls");
        assert_eq!(rx.recv().unwrap(), "X");
    }

    #[test]
    fn unavailable_addon_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut client = GhostClient::new(dir.path().join("missing.sock"));
        assert!(matches!(
            client.preedit("x"),
            Err(GhostError::Unavailable { .. })
        ));
    }

    #[test]
    fn reconnects_after_addon_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let mut client = GhostClient::new(path.clone());
        assert!(client.preedit("x").is_err());

        let rx = fake_addon(path);
        assert_eq!(client.preedit("y").unwrap(), GhostDisplay::Preedit);
        assert_eq!(rx.recv().unwrap(), "P y");
    }
}
