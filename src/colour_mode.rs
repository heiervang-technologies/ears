//! Colour words: saying the colour of a mode on its own switches to it, the
//! same colours the ears LED shows.
//!
//! | said              | mode                                        |
//! |-------------------|---------------------------------------------|
//! | orange            | standby: only the wake phrase is acted on   |
//! | blue, cyan        | dictation: typed where the cursor is        |
//! | yellow            | the talking stick: typing off, stick listens |
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

/// A talking stick listener is running (the yellow mode), as the LED sees it.
pub fn stick_listening() -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return false;
    };
    procs.flatten().any(|p| {
        let Ok(cmdline) = std::fs::read(p.path().join("cmdline")) else {
            return false;
        };
        let argv: Vec<&[u8]> = cmdline.split(|&b| b == 0).take(4).collect();
        let named = argv
            .iter()
            .take(3)
            .any(|a| a.rsplit(|&b| b == b'/').next() == Some(b"talking-stick".as_slice()));
        named && argv.iter().any(|a| *a == b"listen".as_slice())
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
}
