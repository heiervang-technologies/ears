//! Live subtitles: what VAD hears, as it is heard, for an on-screen overlay
//! (the ears LED scene draws it just above its line, e.g. for a stream).
//!
//! While the `subtitles.on` marker exists in the runtime dir, every partial
//! and final transcript is written to `subtitle.json` there:
//!
//! ```json
//! {"seq": 12, "text": "hello wor", "settled": 6, "final": false}
//! ```
//!
//! `settled` is how many bytes of `text` will not change; `final` marks the
//! committed transcript. An empty `text` clears the line (discarded speech).
//! The marker is checked on every write, so `ears subtitles on|off` takes
//! effect in a running VAD at once.

use std::path::{Path, PathBuf};

/// Marker: subtitles are on while this file exists.
pub const MARKER: &str = "subtitles.on";
/// The line the overlay reads.
pub const FILE: &str = "subtitle.json";

#[derive(Debug, Clone)]
pub struct Subtitles {
    marker: PathBuf,
    file: PathBuf,
    seq: u64,
}

impl Subtitles {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            marker: state_dir.join(MARKER),
            file: state_dir.join(FILE),
            seq: 0,
        }
    }

    /// Whether subtitles are on right now.
    pub fn on(&self) -> bool {
        self.marker.exists()
    }

    /// Show `text` with its first `settled` bytes final in all but name.
    pub fn partial(&mut self, text: &str, settled: usize) {
        self.write(text, settled, false);
    }

    /// Show the committed transcript.
    pub fn finished(&mut self, text: &str) {
        self.write(text, text.len(), true);
    }

    /// Take the line down (the utterance produced nothing).
    pub fn clear(&mut self) {
        self.write("", 0, true);
    }

    fn write(&mut self, text: &str, settled: usize, final_: bool) {
        if !self.on() {
            return;
        }
        self.seq += 1;
        let text = text.trim();
        let settled = floor_char_boundary(text, settled.min(text.len()));
        let line = serde_json::json!({
            "seq": self.seq,
            "text": text,
            "settled": text[..settled].chars().count(),
            "final": final_,
        });
        // Atomic: the overlay never reads half a line.
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, line.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

/// Turn subtitles on or off for a running VAD (and the next one).
pub fn set(state_dir: &Path, on: bool) -> std::io::Result<()> {
    let marker = state_dir.join(MARKER);
    if on {
        std::fs::create_dir_all(state_dir)?;
        std::fs::write(marker, b"")
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
        let mut s = Subtitles::new(dir.path());
        s.partial("hello", 3);
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn partial_then_final() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true).unwrap();
        let mut s = Subtitles::new(dir.path());
        s.partial(" hello wor", 6);
        let v = read(dir.path());
        assert_eq!(v["text"], "hello wor");
        assert_eq!(v["settled"], 6);
        assert_eq!(v["final"], false);
        s.finished("hello world");
        let v = read(dir.path());
        assert_eq!(v["seq"], 2);
        assert_eq!(v["settled"], 11);
        assert_eq!(v["final"], true);
    }

    #[test]
    fn settled_counts_characters_not_bytes() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true).unwrap();
        let mut s = Subtitles::new(dir.path());
        s.partial("blåbær er", 3); // inside "å"
        assert_eq!(read(dir.path())["settled"], 2);
    }

    #[test]
    fn off_removes_the_line() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), true).unwrap();
        let mut s = Subtitles::new(dir.path());
        s.finished("hi");
        set(dir.path(), false).unwrap();
        assert!(!dir.path().join(FILE).exists());
        s.finished("again");
        assert!(!dir.path().join(FILE).exists());
        set(dir.path(), false).unwrap();
    }
}
