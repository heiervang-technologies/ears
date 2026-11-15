//! Colour words: saying the colour of a mode on its own switches to it, the
//! same colours the ears LED shows. Each mode also has a name, and saying
//! the name switches to it too (whole utterance only, like the colours).
//!
//! | said                                   | name          | mode                                        |
//! |----------------------------------------|---------------|---------------------------------------------|
//! | orange, passive (mode)                 | passive mode  | standby: only the wake phrase is acted on   |
//! | blue, cyan, write mode, typing (mode), type mode | write mode | dictation: typed where the cursor is |
//! | yellow                                 | stick mode    | the talking stick: typing off, stick listens |
//! | purple, violet                         | command mode  | command mode: routed to actions by embedding |
//! | pink, magenta, vision, look            | vision mode   | vision mode: what is said is found on screen (`ears look`) |
//! | (agents only: talking-stick grab)      | call mode     | green: an agent is calling                  |
//! | (agents only: `ears decide`)           | decision mode | red: a decision (reply) is open             |
//!
//! "stick mode" is not parsed either: configs already have it as a run
//! command (`haios-voice-route stick`), which a name here would shadow.
//! "command mode" is not parsed here: it is [`crate::command_mode::ENTER_PHRASES`],
//! which toggles command mode. Green and red are opened by agents, so they
//! have names ([`CALL_MODE`], [`DECISION_MODE`]) but no words that switch.
//!
//! Blue and yellow go through `haios-voice-route cursor|stick`, which owns
//! the talking stick listener it starts, so blue always stops it again.

/// Green's name: an agent rings (talking-stick grab, clanker calling).
pub const CALL_MODE: &str = "call mode";
/// Red's name: an open `ears decide` (also said "reply mode").
pub const DECISION_MODE: &str = "decision mode";

/// A mode, by the colour the LED shows for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    Orange,
    Blue,
    Yellow,
    Purple,
    Pink,
}

impl Colour {
    /// The colour word or mode name in `heard`, said on its own (a colour
    /// may have " mode" after it).
    pub fn parse(heard: &str) -> Option<Self> {
        let said = crate::commands::normalize(heard);
        // Names, whole: "type" alone stays a word, "type mode" switches.
        match said.as_str() {
            "passive" | "passive mode" => return Some(Self::Orange),
            "write mode" | "type mode" | "typing" | "typing mode" => return Some(Self::Blue),
            _ => {}
        }
        let word = said.strip_suffix(" mode").unwrap_or(&said);
        match word {
            "orange" => Some(Self::Orange),
            "blue" | "cyan" => Some(Self::Blue),
            "yellow" => Some(Self::Yellow),
            "purple" | "violet" => Some(Self::Purple),
            "pink" | "magenta" | "vision" | "look" => Some(Self::Pink),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Orange => "orange",
            Self::Blue => "blue",
            Self::Yellow => "yellow",
            Self::Purple => "purple",
            Self::Pink => "pink",
        }
    }

    /// The mode's name, for people: "write mode" for blue. [`Self::name`]
    /// (the colour) stays what logs, journals and IPC carry.
    pub fn label(self) -> &'static str {
        match self {
            Self::Orange => "passive mode",
            Self::Blue => "write mode",
            Self::Yellow => "stick mode",
            Self::Purple => "command mode",
            Self::Pink => "vision mode",
        }
    }

    /// A colour or a mode name, as `ears colour` takes it: "write", "write
    /// mode", "passive", "blue"... ("command" too, for purple).
    pub fn from_cli(arg: &str) -> Option<Self> {
        let said = crate::commands::normalize(arg);
        match said.as_str() {
            "command" | "command mode" => Some(Self::Purple),
            "write" => Some(Self::Blue),
            "stick" | "stick mode" => Some(Self::Yellow),
            _ => Self::parse(&said),
        }
    }
}

/// Switches where dictation goes (cursor or talking stick), from hai-os.
pub const VOICE_ROUTE: &str = "haios-voice-route";

/// `program` is an executable file somewhere on PATH.
pub fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            std::fs::metadata(dir.join(program))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_words_alone() {
        assert_eq!(Colour::parse("Orange."), Some(Colour::Orange));
        assert_eq!(Colour::parse("cyan"), Some(Colour::Blue));
        assert_eq!(Colour::parse("Yellow mode"), Some(Colour::Yellow));
        assert_eq!(Colour::parse("violet!"), Some(Colour::Purple));
        assert_eq!(Colour::parse("Pink."), Some(Colour::Pink));
        assert_eq!(Colour::parse("vision mode"), Some(Colour::Pink));
        assert_eq!(Colour::parse("a blue car"), None);
        assert_eq!(Colour::parse("blue blue"), None);
    }

    #[test]
    fn mode_names_alone() {
        for said in ["write mode", "Type mode", "typing", "Typing mode."] {
            assert_eq!(Colour::parse(said), Some(Colour::Blue), "{said}");
        }
        for said in ["Passive.", "passive mode"] {
            assert_eq!(Colour::parse(said), Some(Colour::Orange), "{said}");
        }
        // Colours keep working.
        assert_eq!(Colour::parse("blue"), Some(Colour::Blue));
        assert_eq!(Colour::parse("orange mode"), Some(Colour::Orange));
    }

    #[test]
    fn mode_names_only_whole() {
        for said in [
            "type",
            // Sounds like "right" (the arrow key): not a switch.
            "Write.",
            "stick",
            // A run command in the live config ("talking stick" phrases).
            "stick mode",
            "write this down",
            "I am typing",
            "passive voice",
            "write write",
            "command mode",
            "call mode",
            "decision mode",
            "reply mode",
        ] {
            assert_eq!(Colour::parse(said), None, "{said}");
        }
    }

    #[test]
    fn labels_parse_back() {
        for colour in [
            Colour::Orange,
            Colour::Blue,
            Colour::Yellow,
            Colour::Purple,
            Colour::Pink,
        ] {
            assert_eq!(Colour::from_cli(colour.label()), Some(colour));
            assert_eq!(Colour::from_cli(colour.name()), Some(colour));
        }
        assert_eq!(Colour::from_cli("write"), Some(Colour::Blue));
        assert_eq!(Colour::from_cli("passive"), Some(Colour::Orange));
        assert_eq!(Colour::from_cli("command"), Some(Colour::Purple));
        assert_eq!(Colour::from_cli("stick"), Some(Colour::Yellow));
        assert_eq!(Colour::from_cli("call mode"), None);
    }

    #[test]
    fn finds_programs_on_path() {
        assert!(on_path("sh"));
        assert!(!on_path("surely-no-such-program-here"));
    }
}
