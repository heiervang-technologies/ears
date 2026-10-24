//! Colour words: saying the colour of a mode on its own switches to it, the
//! same colours the ears LED shows.
//!
//! | said              | mode                                        |
//! |-------------------|---------------------------------------------|
//! | orange            | standby: only the wake phrase is acted on   |
//! | blue, cyan        | dictation: typed where the cursor is        |
//! | yellow            | the talking stick: typing off, stick listens |
//!
//! Blue and yellow go through `haios-voice-route cursor|stick`, which owns
//! the talking stick listener it starts, so blue always stops it again.
//! | purple, violet    | command mode: routed to actions by embedding |

/// A mode, by the colour the LED shows for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    Orange,
    Blue,
    Yellow,
    Purple,
}

impl Colour {
    /// The colour word in `heard`, said on its own (or "... mode").
    pub fn parse(heard: &str) -> Option<Self> {
        let said = crate::commands::normalize(heard);
        let word = said.strip_suffix(" mode").unwrap_or(&said);
        match word {
            "orange" => Some(Self::Orange),
            "blue" | "cyan" => Some(Self::Blue),
            "yellow" => Some(Self::Yellow),
            "purple" | "violet" => Some(Self::Purple),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Orange => "orange",
            Self::Blue => "blue",
            Self::Yellow => "yellow",
            Self::Purple => "purple",
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
        assert_eq!(Colour::parse("a blue car"), None);
        assert_eq!(Colour::parse("blue blue"), None);
    }

    #[test]
    fn finds_programs_on_path() {
        assert!(on_path("sh"));
        assert!(!on_path("surely-no-such-program-here"));
    }
}
