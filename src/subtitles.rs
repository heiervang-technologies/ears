//! Live subtitles: what VAD hears, as it is heard, for an on-screen overlay
//! (the ears LED scene draws it just above its line, e.g. for a stream).
//!
//! While the `subtitles.on` marker exists in the runtime dir, every partial
//! and final transcript is written to `subtitle.json` there:
//!
//! ```json
//! {"seq": 12, "text": "hello wor", "settled": 6, "final": false, "command": false}
//! ```
//!
//! `settled` is how many characters of `text` will not change; `final` marks
//! the committed transcript; `command` that it was heard as a command (a key,
//! a mode switch, an action), which the overlay highlights. An empty `text` clears the line (discarded speech).
//!
//! In command mode a line says what came of the words: `"action"` names the
//! action that fired when the words were not its name ("send it" -> enter),
//! and `"miss": true` marks words that fired nothing, with `"nearest"` the
//! action they came closest to, if any.
//! The marker is checked on every write, so `ears subtitles on|off` takes
//! effect in a running VAD at once. It holds the modes subtitles show in
//! (config `subtitle_modes`), one per line, so the overlay can show a
//! persistent clue whenever the current mode is captioned.

use std::path::{Path, PathBuf};

/// A VAD mode, by what happens to speech (and its LED colour).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Orange: only the wake phrase acts.
    Standby,
    /// Cyan, yellow, pale blue: dictation, wherever the words go.
    Dictation,
    /// Violet: command mode.
    Command,
    /// Red: a decision is open (`ears decide`).
    Decide,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Standby, Mode::Dictation, Mode::Command, Mode::Decide];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Standby => "standby",
            Mode::Dictation => "dictation",
            Mode::Command => "command",
            Mode::Decide => "decide",
        }
    }
}

/// Marker: subtitles are on while this file exists.
pub const MARKER: &str = "subtitles.on";
/// The line the overlay reads.
pub const FILE: &str = "subtitle.json";

#[derive(Debug, Clone)]
pub struct Subtitles {
    marker: PathBuf,
    /// Modes in which lines are written.
    modes: Vec<Mode>,
    file: PathBuf,
    seq: u64,
}

impl Subtitles {
    pub fn new(state_dir: &Path, modes: &[Mode]) -> Self {
        Self {
            marker: state_dir.join(MARKER),
            modes: modes.to_vec(),
            file: state_dir.join(FILE),
            seq: 0,
        }
    }

    /// Whether subtitles are on right now.
    pub fn on(&self) -> bool {
        self.marker.exists()
    }

    /// Show `text` with its first `settled` bytes final in all but name.
    /// `command`: so far it reads as a command.
    pub fn partial(&mut self, mode: Mode, text: &str, settled: usize, command: bool) {
        if self.modes.contains(&mode) {
            self.write(text, settled, false, command);
        }
    }

    /// Show the committed transcript.
    pub fn finished(&mut self, mode: Mode, text: &str) {
        if self.modes.contains(&mode) {
            self.write(text, text.len(), true, false);
        }
    }

    /// Show what was acted on as a command, highlighted.
    pub fn command(&mut self, mode: Mode, text: &str) {
        if self.modes.contains(&mode) {
            self.write(text, text.len(), true, true);
        }
    }

    /// Take the line down (the utterance produced nothing).
    pub fn clear(&mut self) {
        self.write("", 0, true, false);
    }

    /// Command mode: `heard` fired `action` (the words, highlighted, and
    /// the action when the words were not simply its name).
    pub fn fired(&mut self, mode: Mode, heard: &str, action: &str) {
        if !self.modes.contains(&mode) {
            return;
        }
        let heard = heard.trim();
        let text = if heard.is_empty() { action } else { heard };
        let mut extra = serde_json::Map::new();
        if crate::command_mode::plain(text) != crate::command_mode::plain(action) {
            extra.insert("action".into(), action.into());
        }
        self.write_with(text, text.len(), true, true, extra);
    }

    /// Command mode: `heard` fired nothing; `nearest` is the action it came
    /// closest to. Without words there is nothing to show: the line clears.
    pub fn missed(&mut self, mode: Mode, heard: &str, nearest: Option<&str>) {
        if !self.modes.contains(&mode) {
            return;
        }
        let heard = heard.trim();
        if heard.is_empty() {
            self.clear();
            return;
        }
        let mut extra = serde_json::Map::new();
        extra.insert("miss".into(), true.into());
        if let Some(name) = nearest {
            extra.insert("nearest".into(), name.into());
        }
        self.write_with(heard, heard.len(), true, false, extra);
    }

    fn write(&mut self, text: &str, settled: usize, final_: bool, command: bool) {
        self.write_with(text, settled, final_, command, serde_json::Map::new());
    }

    fn write_with(
        &mut self,
        text: &str,
        settled: usize,
        final_: bool,
        command: bool,
        extra: serde_json::Map<String, serde_json::Value>,
    ) {
        if !self.on() {
            return;
        }
        self.seq += 1;
        let text = text.trim();
        let settled = floor_char_boundary(text, settled.min(text.len()));
        let mut line = serde_json::json!({
            "seq": self.seq,
            "text": text,
            "settled": text[..settled].chars().count(),
            "final": final_,
            "command": command,
        });
        if let Some(map) = line.as_object_mut() {
            map.extend(extra);
        }
        // Atomic: the overlay never reads half a line.
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, line.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

/// Turn subtitles on (in `modes`) or off for a running VAD.
pub fn set(state_dir: &Path, on: bool, modes: &[Mode]) -> std::io::Result<()> {
    let marker = state_dir.join(MARKER);
    if on {
        std::fs::create_dir_all(state_dir)?;
        let list: Vec<&str> = modes.iter().map(|m| m.name()).collect();
        std::fs::write(marker, list.join("\n") + "\n")
    } else {
        let _ = std::fs::remove_file(state_dir.join(FILE));
        match std::fs::remove_file(marker) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

fn floor_char_boundary(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dir: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join(FILE)).unwrap()).unwrap()
    }

    #[test]
    fn writes_nothing_while_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Subtitles::new(dir.path(), &Mode::ALL);
        s.partial(Mode::Dictation, "hello", 3, false);
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn partial_then_final() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true, &Mode::ALL).unwrap();
        let mut s = Subtitles::new(dir.path(), &Mode::ALL);
        s.partial(Mode::Dictation, " hello wor", 6, false);
        let v = read(dir.path());
        assert_eq!(v["text"], "hello wor");
        assert_eq!(v["settled"], 6);
        assert_eq!(v["final"], false);
        s.finished(Mode::Dictation, "hello world");
        let v = read(dir.path());
        assert_eq!(v["seq"], 2);
        assert_eq!(v["settled"], 11);
        assert_eq!(v["final"], true);
    }

    #[test]
    fn settled_counts_characters_not_bytes() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true, &Mode::ALL).unwrap();
        let mut s = Subtitles::new(dir.path(), &Mode::ALL);
        s.partial(Mode::Dictation, "blåbær er", 3, false); // inside "å"
        assert_eq!(read(dir.path())["settled"], 2);
    }

    #[test]
    fn commands_are_marked() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true, &Mode::ALL).unwrap();
        let mut s = Subtitles::new(dir.path(), &Mode::ALL);
        s.command(Mode::Dictation, "enter");
        let v = read(dir.path());
        assert_eq!(v["command"], true);
        assert_eq!(v["final"], true);
        s.finished(Mode::Dictation, "hello");
        assert_eq!(read(dir.path())["command"], false);
    }

    #[test]
    fn only_in_its_modes() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true, &[Mode::Standby]).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(MARKER)).unwrap(),
            "standby\n"
        );
        let mut s = Subtitles::new(dir.path(), &[Mode::Standby]);
        s.finished(Mode::Dictation, "typed");
        assert!(!dir.path().join(FILE).exists());
        s.finished(Mode::Standby, "heard");
        assert_eq!(read(dir.path())["text"], "heard");
    }

    #[test]
    fn off_removes_the_line() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true, &Mode::ALL).unwrap();
        let mut s = Subtitles::new(dir.path(), &Mode::ALL);
        s.finished(Mode::Dictation, "hi");
        set(dir.path(), false, &Mode::ALL).unwrap();
        assert!(!dir.path().join(FILE).exists());
        s.finished(Mode::Dictation, "again");
        assert!(!dir.path().join(FILE).exists());
        set(dir.path(), false, &Mode::ALL).unwrap();
    }

    #[test]
    fn command_lines_say_what_came_of_the_words() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true, &Mode::ALL).unwrap();
        let mut s = Subtitles::new(dir.path(), &Mode::ALL);
        s.fired(Mode::Command, "Send it.", "enter");
        let line = read(dir.path());
        assert_eq!(
            (line["text"].as_str(), line["command"].as_bool()),
            (Some("Send it."), Some(true))
        );
        assert_eq!(line["action"], "enter");
        s.fired(Mode::Command, "Enter.", "enter");
        assert!(
            read(dir.path()).get("action").is_none(),
            "the words were its name"
        );
        s.missed(Mode::Command, "Scroll sideways.", Some("down"));
        let line = read(dir.path());
        assert_eq!(
            (line["miss"].as_bool(), line["nearest"].as_str()),
            (Some(true), Some("down"))
        );
        assert_eq!(line["command"], false);
        s.missed(Mode::Command, "", None);
        assert_eq!(read(dir.path())["text"], "");
    }
}
