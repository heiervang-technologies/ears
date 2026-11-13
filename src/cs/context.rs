//! Prevention: bias the ASR toward the phrases the current mode listens for.
//!
//! In command mode and in standby an utterance only matters if it is one of
//! a closed list of phrases, so those phrases go to the ASR as context (the
//! `prompt` of the HTTP transcription, the system turn of the live decoder).
//! Qwen3-ASR copies spellings it finds in the context: "tmux kill the active
//! pane" instead of "Demux kill the active pain", "hey clanker" instead of
//! "Hey, Klanker!". The context is a plain comma-separated list: the model
//! biases on context, it does not follow instructions in it.
//!
//! Two failure modes come with it, both measured in
//! `experiments/cs-context/`, and both are handled by the caller falling
//! back to a decode without the context:
//!
//! - **empty**: with a context the model answers `language None` (no speech)
//!   for some short phrases it hears fine without one ("enter", "ears off").
//! - **echo**: with the language header forced onto non-speech it writes
//!   the context out verbatim. [`is_echo`] recognizes that.
//!
//! Never in dictation: there every word is meant to be typed, and a list of
//! commands would only pull ordinary speech toward them.

use crate::commands::{normalize, VoiceCommands};
use crate::vad_mode::Mode;

/// Default `[cs] context_chars`: about 115 tokens (1021 characters of
/// command phrases measured 228). The ears stream plugin keeps 256 tokens
/// for the template, the context and its 64-token base when it sizes the
/// longest utterance (`default_max_audio_ms`), so a context much over
/// 170 tokens could push a 90 s utterance past the model length. Measured:
/// caps of 256 and 1024 characters both kept every phrase that fit, and
/// latency did not change measurably up to 4096.
pub const DEFAULT_CAP: usize = 512;

/// Join `phrases` into a context: in order, comma-separated, each once
/// (by [`normalize`]), and only whole phrases while the whole stays within
/// `cap` characters. The list stops at the first phrase that does not fit,
/// so what comes first is what gets in. `None` when nothing fits.
pub fn build<'a>(phrases: impl IntoIterator<Item = &'a str>, cap: usize) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    for phrase in phrases {
        // Commas would split one phrase into two in the list.
        let phrase = phrase.replace(',', " ");
        let phrase = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
        let key = normalize(&phrase);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        let len = out.chars().count() + phrase.chars().count() + if out.is_empty() { 0 } else { 2 };
        if len > cap {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&phrase);
    }
    (!out.is_empty()).then_some(out)
}

/// What standby listens for, most important first: the wake phrases, the
/// standby toggles, "command" (when command mode is set up) and the run
/// commands' phrases. Sleep phrases are not here: they are heard in
/// dictation, which gets no context.
pub fn standby_phrases(commands: &VoiceCommands, command_mode: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if commands.enabled {
        out.extend(commands.wake.iter().cloned());
        out.extend(commands.standby.iter().cloned());
    }
    if command_mode {
        out.extend(
            crate::command_mode::ENTER_PHRASES
                .iter()
                .map(|p| p.to_string()),
        );
    }
    out.extend(
        commands
            .run
            .resolved()
            .into_iter()
            .flat_map(|run| run.phrases),
    );
    out
}

/// What command mode listens for: every action's first phrasing in the
/// order of the actions (built-ins first), then every second phrasing, and
/// so on, so a cap keeps one phrasing of as many actions as it can. The
/// chatter examples of `none` are left out: they are not commands.
pub fn command_phrases(actions: &[crate::command_mode::Action]) -> Vec<String> {
    use crate::command_mode::Effect;
    let actions: Vec<_> = actions
        .iter()
        .filter(|a| !matches!(a.effect, Effect::Nothing))
        .collect();
    let rounds = actions.iter().map(|a| a.text.len()).max().unwrap_or(0);
    (0..rounds)
        .flat_map(|i| actions.iter().filter_map(move |a| a.text.get(i).cloned()))
        .collect()
}

/// The context for an utterance heard in `mode`, if any.
pub fn for_mode(
    mode: &Mode,
    commands: &VoiceCommands,
    command_mode: bool,
    settings: &crate::config::CsConfig,
) -> Option<String> {
    if !settings.context {
        return None;
    }
    let phrases = match mode {
        Mode::Dictation | Mode::Vision => return None,
        Mode::Standby => standby_phrases(commands, command_mode),
        Mode::Command(state) => command_phrases(&state.mode.router.actions),
    };
    build(phrases.iter().map(String::as_str), settings.context_chars)
}

/// Whether `text` is the context written out rather than something heard:
/// a run of the context's words longer than any one of its phrases. A
/// single phrase from the list is a legitimate hit, not an echo.
pub fn is_echo(text: &str, context: &str) -> bool {
    let said = normalize(text);
    if said.is_empty() {
        return false;
    }
    let longest = context
        .split(',')
        .map(|p| normalize(p).split_whitespace().count())
        .max()
        .unwrap_or(0);
    let all = format!(" {} ", normalize(context));
    said.split_whitespace().count() > longest && all.contains(&format!(" {said} "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CsConfig;

    fn settings() -> CsConfig {
        CsConfig::default()
    }

    #[test]
    fn build_joins_whole_phrases_within_the_cap() {
        let list = ["ears on", "hey clanker", "next track"];
        assert_eq!(
            build(list, 100).as_deref(),
            Some("ears on, hey clanker, next track")
        );
        // "ears on, hey clanker" is 20 characters: the third does not fit.
        assert_eq!(build(list, 20).as_deref(), Some("ears on, hey clanker"));
        assert_eq!(build(list, 19).as_deref(), Some("ears on"));
        assert_eq!(build(list, 6), None);
        assert_eq!(build([], 100), None);
    }

    #[test]
    fn build_stops_at_the_first_phrase_that_does_not_fit() {
        // Order is priority: a short phrase later does not jump the queue.
        let list = ["ears on", "a much longer phrase than fits", "up"];
        assert_eq!(build(list, 15).as_deref(), Some("ears on"));
    }

    #[test]
    fn build_dedupes_and_cleans() {
        let list = ["Ears on", "ears on.", "  ", "toggle,  window", "ears   off"];
        assert_eq!(
            build(list, 100).as_deref(),
            Some("Ears on, toggle window, ears off")
        );
    }

    #[test]
    fn build_counts_characters_not_bytes() {
        assert_eq!(build(["kopiër"], 6).as_deref(), Some("kopiër"));
    }

    fn commands() -> VoiceCommands {
        toml::from_str(
            r#"
            enabled = true
            wake = ["ears on", "hey clanker"]
            standby = ["ears vad"]
            sleep = ["ears off", "yourself"]
            [run.lights]
            phrases = ["lights on"]
            command = ["lights"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn standby_listens_for_wake_toggle_command_and_run_phrases_not_sleep() {
        let phrases = standby_phrases(&commands(), true);
        let pos = |p: &str| phrases.iter().position(|x| x == p);
        assert_eq!(pos("ears on"), Some(0));
        assert_eq!(pos("hey clanker"), Some(1));
        assert_eq!(pos("ears vad"), Some(2));
        assert!(pos("command").is_some());
        assert!(pos("lights on").is_some());
        assert!(pos("ears off").is_none());
        assert!(pos("yourself").is_none());
        // Without command mode set up, "command" does nothing in standby.
        assert!(!standby_phrases(&commands(), false).contains(&"command".to_string()));
    }

    #[test]
    fn standby_without_commands_keeps_only_run_phrases() {
        let mut c = commands();
        c.enabled = false;
        let phrases = standby_phrases(&c, false);
        assert!(!phrases.contains(&"ears on".to_string()));
        assert!(phrases.contains(&"lights on".to_string()));
    }

    fn actions() -> Vec<crate::command_mode::Action> {
        crate::command_mode::parse_actions(
            r#"
            [[action]]
            name = "enter"
            keys = "enter"
            text = ["enter", "press enter", "submit"]

            [[action]]
            name = "music"
            keys = "space"
            text = ["pause the music"]
            "#,
        )
        .unwrap()
        .1
    }

    #[test]
    fn command_mode_takes_first_phrasings_first_and_skips_chatter() {
        let phrases = command_phrases(&actions());
        // Built-ins come first: "leave" is the way out.
        assert_eq!(phrases[0], "command");
        let first = phrases.iter().position(|p| p == "enter").unwrap();
        let music = phrases.iter().position(|p| p == "pause the music").unwrap();
        let second = phrases.iter().position(|p| p == "press enter").unwrap();
        assert!(first < music && music < second);
        assert!(!phrases.iter().any(|p| p == "hmm let me think"));
    }

    #[test]
    fn context_only_in_standby_and_command_mode() {
        let c = commands();
        let s = settings();
        assert_eq!(for_mode(&Mode::Dictation, &c, true, &s), None);
        assert_eq!(for_mode(&Mode::Vision, &c, true, &s), None);
        let standby = for_mode(&Mode::Standby, &c, true, &s).unwrap();
        assert!(
            standby.starts_with("ears on, hey clanker, ears vad, "),
            "{standby}"
        );
        assert!(standby.contains(", command, "), "{standby}");
        assert!(!standby.contains("yourself"));
        let off = CsConfig {
            context: false,
            ..settings()
        };
        assert_eq!(for_mode(&Mode::Standby, &c, true, &off), None);
    }

    #[test]
    fn context_respects_the_configured_cap() {
        let s = CsConfig {
            context_chars: 20,
            ..settings()
        };
        let context = for_mode(&Mode::Standby, &commands(), true, &s).unwrap();
        assert_eq!(context, "ears on, hey clanker");
        let default = for_mode(&Mode::Standby, &commands(), true, &settings()).unwrap();
        assert!(default.chars().count() <= DEFAULT_CAP);
    }

    #[test]
    fn echo_is_a_run_of_context_longer_than_any_phrase() {
        let context = "ears on, ears off, hey clanker, listen clanker";
        // Measured on noise with the language header forced.
        assert!(is_echo("Ears on, ears off, hey clanker, listen", context));
        assert!(is_echo("ears off hey clanker", context));
        // One phrase is a hit, not an echo.
        assert!(!is_echo("Hey clanker.", context));
        assert!(!is_echo("ears on", context));
        // Something else entirely.
        assert!(!is_echo("the clanker said ears on twice", context));
        assert!(!is_echo("", context));
    }
}
