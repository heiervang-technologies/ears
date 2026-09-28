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

/// What happened to a final transcript handed to the addon.
///
/// The addon commits before it replies, so a lost reply does not mean the
/// text was not delivered. Only outcomes where the commit certainly did not
/// happen may be retried by typing; retrying an [`Delivery::Unknown`] could
/// put the text in twice, possibly into another window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The focused application received the text.
    Delivered,
    /// Nothing was committed; the caller should type the text instead.
    NotDelivered,
    /// The commit may or may not have happened. Do not retry.
    Unknown,
}

/// Class of the window Hyprland has focused, if it can be asked.
pub fn hyprland_active_class() -> Option<String> {
    let out = std::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("class")?.as_str().map(str::to_string)
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

/// PCM payload of a WAV file that may still be being written.
///
/// `pw-record` only fills in the RIFF/data sizes when it closes the file, so
/// the sizes are ignored: everything after the `data` chunk header up to the
/// end of what has been written so far is returned, trimmed to whole 16-bit
/// samples.
pub fn growing_wav_payload(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        if id == b"data" {
            let body = &bytes[pos + 8..];
            return Some(&body[..body.len() & !1]);
        }
        // Chunks are word-aligned.
        pos = pos.checked_add(8 + size + (size & 1))?;
    }
    None
}

/// Write mono 16-bit PCM as a WAV file.
pub fn write_pcm16_wav(
    path: &std::path::Path,
    pcm: &[u8],
    sample_rate: u32,
) -> std::io::Result<()> {
    let data_len = pcm.len() as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    std::fs::write(path, out)
}

/// Client for the `earsghost` fcitx5 addon.
///
/// Connects lazily and reconnects after any failure, so fcitx5 restarts are
/// tolerated. Dropping the client closes the connection, which makes the
/// addon clear any ghost still on screen.
pub struct GhostClient {
    path: PathBuf,
    conn: Option<(UnixStream, BufReader<UnixStream>)>,
    active_class: fn() -> Option<String>,
}

impl GhostClient {
    pub fn new(path: PathBuf) -> Self {
        Self::with_focus_probe(path, hyprland_active_class)
    }

    /// Client that asks `active_class` which window really has focus.
    pub fn with_focus_probe(path: PathBuf, active_class: fn() -> Option<String>) -> Self {
        Self {
            path,
            conn: None,
            active_class,
        }
    }

    /// Is the addon reachable right now?
    pub fn probe(&mut self) -> Result<GhostDisplay, GhostError> {
        self.send("S").map(|(shown, _)| shown)
    }

    /// Does the addon's input context belong to the window that has focus?
    ///
    /// fcitx5 keeps reporting the last app as focused when focus moves to a
    /// surface that never enables text input, so its answer alone would put
    /// ghost text (or the final commit) into a window the user left. When
    /// either side cannot tell, trust the addon.
    fn focus_matches(&mut self) -> Result<bool, GhostError> {
        let (shown, program) = self.send("S")?;
        if shown == GhostDisplay::None {
            return Ok(false);
        }
        let Some(program) = program else {
            return Ok(true);
        };
        Ok((self.active_class)().is_none_or(|class| program.eq_ignore_ascii_case(&class)))
    }

    /// Show `text` as the ghost, replacing the previous one. Returns
    /// [`GhostDisplay::None`] (and clears any ghost) when the focused window
    /// cannot show it.
    pub fn preedit(&mut self, text: &str) -> Result<GhostDisplay, GhostError> {
        if !self.focus_matches()? {
            self.send("X")?;
            return Ok(GhostDisplay::None);
        }
        self.send(&format!("P {}", escape(text)))
            .map(|(shown, _)| shown)
    }

    /// Clear the ghost and deliver `text` to the focused application.
    pub fn commit(&mut self, text: &str) -> Delivery {
        match self.focus_matches() {
            Ok(true) => {}
            Ok(false) => {
                let _ = self.send("X");
                return Delivery::NotDelivered;
            }
            // Only a status query was sent: nothing can have been committed.
            Err(_) => return Delivery::NotDelivered,
        }
        match self.send(&format!("C {}", escape(text))) {
            Ok((GhostDisplay::None, _)) => Delivery::NotDelivered,
            Ok(_) => Delivery::Delivered,
            // Could not even connect: the command never left.
            Err(GhostError::Unavailable { .. }) => Delivery::NotDelivered,
            Err(_) => Delivery::Unknown,
        }
    }

    /// Clear the ghost without delivering anything.
    pub fn clear(&mut self) -> Result<GhostDisplay, GhostError> {
        self.send("X").map(|(shown, _)| shown)
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

    fn send(&mut self, line: &str) -> Result<(GhostDisplay, Option<String>), GhostError> {
        self.connect()?;
        let result = self.exchange(line);
        if result.is_err() {
            // Any I/O failure invalidates the stream; reconnect next time.
            self.conn = None;
        }
        result
    }

    fn exchange(&mut self, line: &str) -> Result<(GhostDisplay, Option<String>), GhostError> {
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

/// Parse "OK <display> [program]".
fn parse_reply(reply: &str) -> Result<(GhostDisplay, Option<String>), GhostError> {
    let mut words = reply.splitn(3, ' ');
    let shown = match (words.next(), words.next()) {
        (Some("OK"), Some("preedit")) => GhostDisplay::Preedit,
        (Some("OK"), Some("panel")) => GhostDisplay::Panel,
        (Some("OK"), _) => GhostDisplay::None,
        _ => return Err(GhostError::Rejected(reply.to_string())),
    };
    let program = words
        .next()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    Ok((shown, program))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    /// Minimal stand-in for the addon: records lines, reports "TestApp" as
    /// focused and answers "OK preedit". With `drop_on_commit` it hangs up
    /// on `C` without replying, like an addon that crashed mid-commit.
    fn fake_addon_with(path: PathBuf, drop_on_commit: bool) -> mpsc::Receiver<String> {
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut writer = stream.try_clone().unwrap();
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let commit = line.starts_with('C');
                    let reply: &[u8] = if line == "S" {
                        b"OK preedit TestApp\n"
                    } else {
                        b"OK preedit\n"
                    };
                    let _ = tx.send(line);
                    if commit && drop_on_commit {
                        break;
                    }
                    let _ = writer.write_all(reply);
                }
            }
        });
        rx
    }

    fn fake_addon(path: PathBuf) -> mpsc::Receiver<String> {
        fake_addon_with(path, false)
    }

    fn focus_on_test_app() -> Option<String> {
        Some("testapp".to_string())
    }

    fn focus_elsewhere() -> Option<String> {
        Some("hover".to_string())
    }

    fn client(path: PathBuf) -> GhostClient {
        GhostClient::with_focus_probe(path, focus_on_test_app)
    }

    /// Commands the client sent, minus the status queries around them.
    fn commands(rx: &mpsc::Receiver<String>) -> Vec<String> {
        rx.try_iter().filter(|l| l != "S").collect()
    }

    #[test]
    fn growing_wav_payload_ignores_unfinished_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_pcm16_wav(&path, &[1, 0, 2, 0, 3, 0], 16000).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        // Simulate pw-record mid-write: zero sizes and an odd trailing byte.
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        bytes.push(9);
        assert_eq!(growing_wav_payload(&bytes).unwrap(), &[1, 0, 2, 0, 3, 0]);
    }

    #[test]
    fn growing_wav_payload_skips_extra_chunks() {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[7, 7, 7, 0]); // 3 bytes + pad
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[5, 6]);
        assert_eq!(growing_wav_payload(&bytes).unwrap(), &[5, 6]);
        assert!(growing_wav_payload(b"nope").is_none());
    }

    #[test]
    fn escape_roundtrips_specials() {
        assert_eq!(escape("a\\b\nc\r"), "a\\\\b\\nc");
        assert_eq!(escape("plain text"), "plain text");
    }

    #[test]
    fn parse_replies() {
        assert_eq!(
            parse_reply("OK preedit").unwrap(),
            (GhostDisplay::Preedit, None)
        );
        assert_eq!(parse_reply("OK panel").unwrap().0, GhostDisplay::Panel);
        assert_eq!(parse_reply("OK none").unwrap().0, GhostDisplay::None);
        assert_eq!(
            parse_reply("OK preedit Alacritty").unwrap(),
            (GhostDisplay::Preedit, Some("Alacritty".to_string()))
        );
        assert!(parse_reply("ERR nope").is_err());
        assert!(parse_reply("").is_err());
    }

    #[test]
    fn client_speaks_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = client(path);

        assert_eq!(client.preedit("hello wor").unwrap(), GhostDisplay::Preedit);
        assert_eq!(client.commit("hello world\nls"), Delivery::Delivered);
        client.clear().unwrap();

        assert_eq!(commands(&rx), ["P hello wor", "C hello world\\nls", "X"]);
    }

    #[test]
    fn focus_mismatch_types_instead_of_committing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = GhostClient::with_focus_probe(path, focus_elsewhere);

        assert_eq!(client.preedit("hi").unwrap(), GhostDisplay::None);
        assert_eq!(client.commit("hi"), Delivery::NotDelivered);
        assert_eq!(commands(&rx), ["X", "X"]);
    }

    #[test]
    fn lost_commit_reply_is_unknown_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let _rx = fake_addon_with(path.clone(), true);
        let mut client = client(path);
        assert_eq!(client.commit("hi"), Delivery::Unknown);
    }

    #[test]
    fn unreachable_addon_is_not_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let mut client = client(dir.path().join("missing.sock"));
        assert_eq!(client.commit("hi"), Delivery::NotDelivered);
    }

    #[test]
    fn unavailable_addon_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut client = client(dir.path().join("missing.sock"));
        assert!(matches!(
            client.preedit("x"),
            Err(GhostError::Unavailable { .. })
        ));
    }

    #[test]
    fn reconnects_after_addon_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let mut client = client(path.clone());
        assert!(client.preedit("x").is_err());

        let rx = fake_addon(path);
        assert_eq!(client.preedit("y").unwrap(), GhostDisplay::Preedit);
        assert_eq!(commands(&rx), ["P y"]);
    }
}
