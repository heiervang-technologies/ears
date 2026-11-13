//! The VAD mode: what an utterance is for, in one value.
//!
//! | mode      | LED    | an utterance is                                  |
//! |-----------|--------|--------------------------------------------------|
//! | dictation | blue   | typed (yellow when typing is off, see below)     |
//! | standby   | orange | ignored, except wake phrases and run commands    |
//! | command   | violet | routed to an action ([`crate::command_mode`])    |
//! | vision    | pink   | looked for on screen (`ears look`)               |
//!
//! The modes exclude each other: [`Mode`] holds exactly one, so ears can no
//! longer stand by while still in command or vision mode. Two things stay
//! outside it on purpose:
//!
//! - an open decision (red, `ears decide`): it takes every utterance in any
//!   mode and closes back into that mode, so it lies over the mode. Its asker
//!   owns `decision.json`.
//! - where dictation goes (typed, typing off for the talking stick, claimed,
//!   suspended after a typing failure): that is the output, not the mode.
//!
//! The engine changes mode in one place (`StreamingEngine::set_mode`), which
//! leaves the old mode, enters the new one and publishes it. The files other
//! programs read are derived from [`ModeKind`] alone ([`ModeKind::markers`]):
//! the old one-file-per-mode markers, kept for the LED (`haios-ears-led`),
//! `ears-motion`, `ears prompt` and `ears standby|command status`, and
//! [`FILE`], the whole mode in one file.

use crate::wake::Wake;
use std::path::{Path, PathBuf};

/// The whole mode in one file in the state dir: `{"mode":"command"}`.
/// Written while VAD runs; like the markers, it may outlive a VAD that
/// died, so readers check `vad.pid` first.
pub const FILE: &str = "mode.json";

/// A mode, without its state: what is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeKind {
    Dictation,
    Standby,
    Command,
    Vision,
}

/// Command mode's state while it is on. Sub-states of command mode go here
/// (chanting, ears#264), so they end when command mode does.
pub struct CommandState {
    pub mode: crate::command_mode::CommandMode,
}

/// The mode VAD is in.
#[derive(Default)]
pub enum Mode {
    /// Utterances are dictated.
    #[default]
    Dictation,
    /// Listening for the wake phrase only (see [`crate::wake`]).
    Standby,
    /// Utterances route to actions by embedding.
    Command(Box<CommandState>),
    /// Utterances go to `ears look`.
    Vision,
}

impl Mode {
    pub fn kind(&self) -> ModeKind {
        match self {
            Mode::Dictation => ModeKind::Dictation,
            Mode::Standby => ModeKind::Standby,
            Mode::Command(_) => ModeKind::Command,
            Mode::Vision => ModeKind::Vision,
        }
    }

    pub fn is_standby(&self) -> bool {
        matches!(self, Mode::Standby)
    }

    pub fn is_command(&self) -> bool {
        matches!(self, Mode::Command(_))
    }

    pub fn is_vision(&self) -> bool {
        matches!(self, Mode::Vision)
    }

    /// Command mode's state, while it is on.
    pub fn command_mut(&mut self) -> Option<&mut crate::command_mode::CommandMode> {
        match self {
            Mode::Command(state) => Some(&mut state.mode),
            _ => None,
        }
    }
}

/// One of the old marker files: its name in the state dir, and whether it
/// exists in a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// `vad.standby` ([`crate::prompt::STANDBY_MARKER`]).
    Standby,
    /// `vad.command` ([`crate::command_mode::MARKER`]).
    Command,
    /// `vad.vision` ([`crate::streaming_engine::VISION_MARKER`]).
    Vision,
}

impl Marker {
    pub const ALL: [Marker; 3] = [Marker::Standby, Marker::Command, Marker::Vision];

    pub fn name(self) -> &'static str {
        match self {
            Marker::Standby => crate::prompt::STANDBY_MARKER,
            Marker::Command => crate::command_mode::MARKER,
            Marker::Vision => crate::streaming_engine::VISION_MARKER,
        }
    }
}

impl ModeKind {
    pub fn name(self) -> &'static str {
        match self {
            ModeKind::Dictation => "dictation",
            ModeKind::Standby => "standby",
            ModeKind::Command => "command",
            ModeKind::Vision => "vision",
        }
    }

    /// Whether `marker` exists in this mode: each mode but dictation has
    /// its own, and only one exists at a time.
    pub fn marks(self, marker: Marker) -> bool {
        matches!(
            (self, marker),
            (ModeKind::Standby, Marker::Standby)
                | (ModeKind::Command, Marker::Command)
                | (ModeKind::Vision, Marker::Vision)
        )
    }

    /// The markers that exist in this mode.
    pub fn markers(self) -> Vec<Marker> {
        Marker::ALL.into_iter().filter(|m| self.marks(*m)).collect()
    }

    /// The mode a wake phrase (or `ears standby on|off|toggle`) leads to.
    /// Standby is left for dictation; any other mode is left for standby.
    /// The mode it is already in, when it asks for that (the phrase only
    /// repeats its tone): "ears on" in command mode stays there.
    pub fn after_wake(self, wake: Wake) -> ModeKind {
        let standby = self == ModeKind::Standby;
        match (wake.standby_after(standby), standby) {
            (true, true) | (false, false) => self,
            (true, false) => ModeKind::Standby,
            (false, true) => ModeKind::Dictation,
        }
    }

    /// The mode a colour word leads to (blue and yellow are both dictation:
    /// they differ in where it goes, the output).
    pub fn of_colour(colour: crate::colour_mode::Colour) -> ModeKind {
        use crate::colour_mode::Colour;
        match colour {
            Colour::Orange => ModeKind::Standby,
            Colour::Blue | Colour::Yellow => ModeKind::Dictation,
            Colour::Purple => ModeKind::Command,
            Colour::Pink => ModeKind::Vision,
        }
    }

    /// [`FILE`]'s contents in this mode.
    pub fn json(self) -> String {
        serde_json::json!({ "mode": self }).to_string()
    }
}

/// Where the mode is published. Each file is optional: `ears ws-listen`
/// publishes none, and tests only some.
#[derive(Debug, Default)]
pub struct ModeFiles {
    /// `vad.standby`, for `ears prompt` and the LED.
    pub standby: Option<PathBuf>,
    /// The state dir, for `vad.command`, `vad.vision` and [`FILE`].
    pub dir: Option<PathBuf>,
}

impl ModeFiles {
    fn path(&self, marker: Marker) -> Option<PathBuf> {
        match marker {
            Marker::Standby => self.standby.clone(),
            Marker::Command | Marker::Vision => self.dir.as_ref().map(|d| d.join(marker.name())),
        }
    }

    /// Make the files show `kind`. A marker that already shows it is left
    /// alone; [`FILE`] is replaced whole, so a reader never sees half of it.
    pub fn publish(&self, kind: ModeKind) {
        for marker in Marker::ALL {
            let Some(path) = self.path(marker) else {
                continue;
            };
            let result = match (kind.marks(marker), path.exists()) {
                (true, false) => std::fs::write(&path, b""),
                (false, true) => std::fs::remove_file(&path),
                _ => Ok(()),
            };
            if let Err(e) = result {
                tracing::warn!("Cannot update the mode marker {}: {}", path.display(), e);
            }
        }
        if let Some(dir) = &self.dir {
            if let Err(e) = write_atomic(&dir.join(FILE), kind.json().as_bytes()) {
                tracing::warn!("Cannot write {}: {}", dir.join(FILE).display(), e);
            }
        }
    }

    /// The state dir, where a running `ears look` is found.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// The mode [`FILE`] in `dir` shows, if it is there and reads.
pub fn read(dir: &Path) -> Option<ModeKind> {
    #[derive(serde::Deserialize)]
    struct File {
        mode: ModeKind,
    }
    let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
    serde_json::from_str::<File>(&text).ok().map(|f| f.mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colour_mode::Colour;
    use ModeKind::*;

    const KINDS: [ModeKind; 4] = [Dictation, Standby, Command, Vision];

    #[test]
    fn wake_transitions() {
        // (from, wake, to)
        let table = [
            (Dictation, Wake::Off, Standby),
            (Dictation, Wake::On, Dictation),
            (Dictation, Wake::Toggle, Standby),
            (Standby, Wake::Off, Standby),
            (Standby, Wake::On, Dictation),
            (Standby, Wake::Toggle, Dictation),
            (Command, Wake::Off, Standby),
            (Command, Wake::On, Command),
            (Command, Wake::Toggle, Standby),
            (Vision, Wake::Off, Standby),
            (Vision, Wake::On, Vision),
            (Vision, Wake::Toggle, Standby),
        ];
        for (from, wake, to) in table {
            assert_eq!(from.after_wake(wake), to, "{from:?} + {wake:?}");
        }
    }

    #[test]
    fn colour_transitions() {
        let table = [
            (Colour::Orange, Standby),
            (Colour::Blue, Dictation),
            (Colour::Yellow, Dictation),
            (Colour::Purple, Command),
            (Colour::Pink, Vision),
        ];
        for (colour, to) in table {
            assert_eq!(ModeKind::of_colour(colour), to, "{colour:?}");
        }
    }

    #[test]
    fn each_mode_has_at_most_one_marker() {
        let names = |k: ModeKind| {
            k.markers()
                .into_iter()
                .map(Marker::name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Dictation), Vec::<&str>::new());
        assert_eq!(names(Standby), vec!["vad.standby"]);
        assert_eq!(names(Command), vec!["vad.command"]);
        assert_eq!(names(Vision), vec!["vad.vision"]);
    }

    #[test]
    fn publish_writes_markers_and_mode_json() {
        let dir = tempfile::tempdir().unwrap();
        let files = ModeFiles {
            standby: Some(dir.path().join("vad.standby")),
            dir: Some(dir.path().to_path_buf()),
        };
        for kind in KINDS.into_iter().chain(KINDS.into_iter().rev()) {
            files.publish(kind);
            for marker in Marker::ALL {
                assert_eq!(
                    dir.path().join(marker.name()).exists(),
                    kind.marks(marker),
                    "{kind:?}: {marker:?}"
                );
            }
            assert_eq!(read(dir.path()), Some(kind));
            let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
            assert_eq!(text, format!("{{\"mode\":\"{}\"}}", kind.name()));
        }
        assert!(!dir.path().join("mode.json.tmp").exists());
    }

    #[test]
    fn publish_without_a_dir_touches_only_the_standby_marker() {
        let dir = tempfile::tempdir().unwrap();
        let files = ModeFiles {
            standby: Some(dir.path().join("vad.standby")),
            dir: None,
        };
        files.publish(Standby);
        files.publish(Command);
        files.publish(Standby);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["vad.standby"]);
    }
}
