//! `ears cs migrate`: turn the hand-written mishearing variants in a user's
//! `config.toml` (and `actions.toml`) into a reviewable cs file, without
//! touching either.
//!
//! Each phrase list is split into canonical phrases and variants. The
//! built-in phrases of a `[commands]` list ("ears on" for `wake`) start out
//! canonical; otherwise the first entry is. Going down the list, an entry
//! becomes a variant of the nearest phrase already seen when one of these
//! holds, and stays canonical otherwise (when unsure, it stays canonical):
//!
//! - letters spelled out that make up the phrase: "h e y clanker" (spelled);
//! - the same letters, split or joined: "ad sense", "adsense" (split);
//! - the phrase with only filler dropped ("clanker" for "hey clanker"), or
//!   with words cut to their initial ("f off" for "fuck off") (truncation);
//! - the same number of words, each differing word close in sound
//!   ([`NEAR_WORD`]) and the whole close in sound ([`NEAR`], on keys of at
//!   least [`MIN_KEY`] sounds): a differing common word is a homophone
//!   ("years off"), a differing name is name drift ("klinker");
//! - a different number of words, no word or stem shared, and close in sound
//!   as a whole:
//!   "yourself" for "ears off", "century on" for "centurion" (homophone).
//!
//! Never a variant: a phrase whose words contain the other's in order with
//! more than filler between them ("peace out" and "peace", "cursor mode" and
//! "cursor", "scroll to the top" and "scroll to top"), a reworded one that
//! shares words ("make it fullscreen"), and a written form of the same
//! words ("ctrl" for "control", "4" for "four"). Variants are found against
//! variants too, and take their phrase's meaning.

use super::risk::{phonetic_distance, phonetic_key};
use super::{CsFile, Entry, FailureMode, Meta, Source, Status, COMMAND_TARGETS};
use crate::commands::{normalize, VoiceCommands};

/// Phonetic distance at or below which two phrases count as the same sound.
pub const NEAR: f32 = 0.38;
/// A differing word must be at least this close to the one it replaces.
pub const NEAR_WORD: f32 = 0.3;
/// Keys (of `risk::phonetic_key`) shorter than this must match exactly.
pub const MIN_KEY: usize = 4;

/// Words dropped in a truncation: the phrase still means the same without
/// them.
const FILLER: &[&str] = &[
    "hey", "hi", "ok", "okay", "the", "a", "an", "please", "oh", "uh", "um", "so", "now",
];

/// Common English words: a differing one makes a homophone, any other word
/// is taken for a name (name drift).
const COMMON: &[&str] = &[
    "a", "about", "after", "again", "all", "an", "and", "are", "as", "at", "back", "be", "by",
    "call", "can", "close", "come", "day", "do", "down", "ears", "enter", "for", "from", "full",
    "get", "go", "good", "hay", "he", "help", "her", "here", "hey", "hi", "his", "how", "i", "if",
    "in", "is", "it", "just", "left", "like", "line", "listen", "make", "me", "mode", "more",
    "move", "my", "new", "next", "no", "normal", "not", "now", "of", "off", "ok", "okay", "on",
    "one", "or", "our", "out", "over", "peace", "piece", "play", "press", "regular", "right",
    "say", "scratch", "see", "send", "show", "so", "stick", "stop", "take", "talk", "that", "the",
    "their", "them", "then", "there", "they", "this", "time", "to", "two", "type", "up", "us",
    "was", "we", "what", "when", "which", "who", "will", "with", "would", "years", "yes", "you",
    "your", "yourself",
];

/// A list's phrases sorted into canonical ones and variants.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub target: String,
    /// Phrases of the list that stay canonical (seeds not included).
    pub canonical: Vec<String>,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub heard: String,
    pub meant: String,
    pub mode: FailureMode,
}

/// Sort `list` into canonical phrases and variants. `seeds` are canonical
/// from the start (built-in phrases); entries equal to one already seen are
/// dropped as duplicates.
pub fn split_list(target: &str, seeds: &[String], list: &[String]) -> Split {
    // (phrase, what it means): a canonical phrase means itself.
    let mut known: Vec<(String, String)> = seeds
        .iter()
        .filter(|s| !normalize(s).is_empty())
        .map(|s| (s.clone(), s.clone()))
        .collect();
    let mut split = Split {
        target: target.to_string(),
        canonical: Vec::new(),
        variants: Vec::new(),
    };
    for phrase in list {
        let said = normalize(phrase);
        if said.is_empty() || known.iter().any(|(k, _)| normalize(k) == said) {
            continue;
        }
        let best = known
            .iter()
            .filter_map(|(k, meant)| relate(&said, &normalize(k)).map(|(m, d)| (m, d, meant)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((mode, _, meant)) => {
                let meant = meant.clone();
                split.variants.push(Variant {
                    heard: phrase.clone(),
                    meant: meant.clone(),
                    mode,
                });
                known.push((phrase.clone(), meant));
            }
            None => {
                split.canonical.push(phrase.clone());
                known.push((phrase.clone(), phrase.clone()));
            }
        }
    }
    split
}

/// Whether `heard` is a variant of `meant` (both normalized), how, and how
/// far apart they sound (lower is nearer).
pub fn relate(heard: &str, meant: &str) -> Option<(FailureMode, f32)> {
    let hw: Vec<&str> = heard.split(' ').collect();
    let mw: Vec<&str> = meant.split(' ').collect();

    if let Some(joined) = join_letters(&hw) {
        let joined = normalize(&joined);
        if joined == meant || joined.replace(' ', "") == meant.replace(' ', "") {
            return Some((FailureMode::Spelled, 0.0));
        }
    }
    if hw.len() != mw.len() && heard.replace(' ', "") == meant.replace(' ', "") {
        return Some((FailureMode::Split, 0.0));
    }
    // One inside the other, in order: a shorter phrase that only drops
    // filler, else a phrase of its own.
    if hw.len() < mw.len() {
        if let Some(dropped) = dropped_from(&mw, &hw) {
            return dropped
                .iter()
                .all(|w| FILLER.contains(w))
                .then_some((FailureMode::Truncation, 0.1));
        }
    }
    if hw.len() > mw.len() && dropped_from(&hw, &mw).is_some() {
        return None;
    }
    if hw.len() == mw.len()
        && hw.iter().zip(&mw).all(|(h, m)| m.starts_with(h))
        && hw.iter().zip(&mw).any(|(h, m)| h.len() == 1 && m.len() > 1)
    {
        return Some((FailureMode::Truncation, 0.1));
    }
    if hw.len() == mw.len()
        && hw
            .iter()
            .zip(&mw)
            .all(|(h, m)| h == m || written_alike(h, m))
    {
        return None;
    }

    let (hk, mk) = (phonetic_key(heard), phonetic_key(meant));
    let distance = phonetic_distance(heard, meant);
    let long_enough = hk.len().max(mk.len()) >= MIN_KEY;
    if distance > NEAR || (distance > 0.0 && !long_enough) || hk.is_empty() {
        return None;
    }
    if hw.len() == mw.len() {
        let differing: Vec<(&str, &str)> = hw
            .iter()
            .zip(&mw)
            .filter(|(h, m)| h != m)
            .map(|(h, m)| (*h, *m))
            .collect();
        if differing
            .iter()
            .any(|(h, m)| phonetic_distance(h, m) > NEAR_WORD)
        {
            return None;
        }
        let mode = if differing.iter().all(|(_, m)| COMMON.contains(m)) {
            FailureMode::Homophone
        } else {
            FailureMode::NameDrift
        };
        return Some((mode, distance));
    }
    let (hj, mj) = (heard.replace(' ', ""), meant.replace(' ', ""));
    let same_stem =
        |a: &str, b: &str| a.len().min(b.len()) >= 4 && (a.starts_with(b) || b.starts_with(a));
    if hw.iter().any(|h| mw.iter().any(|m| same_stem(h, m))) || hj.contains(&mj) || mj.contains(&hj)
    {
        // Reworded, not misheard: a shared word or stem ("launcher", "launch
        // apps"), or one inside the other ("make it fullscreen", "full screen").
        return None;
    }
    Some((FailureMode::Homophone, distance))
}

/// Two ways to write one word, not a mishearing.
fn written_alike(a: &str, b: &str) -> bool {
    const SAME: &[&[&str]] = &[
        &["ctrl", "control"],
        &["alt", "alternate"],
        &["esc", "escape"],
        &["del", "delete"],
        &["0", "zero"],
        &["1", "one"],
        &["2", "two"],
        &["3", "three"],
        &["4", "four"],
        &["5", "five"],
        &["6", "six"],
        &["7", "seven"],
        &["8", "eight"],
        &["9", "nine"],
        &["10", "ten"],
    ];
    SAME.iter()
        .any(|forms| forms.contains(&a) && forms.contains(&b))
}

/// The words of `big` left over when `small` is found in it in order, or
/// None if it is not.
fn dropped_from<'a>(big: &[&'a str], small: &[&str]) -> Option<Vec<&'a str>> {
    let mut rest = small.iter().peekable();
    let mut dropped = Vec::new();
    for word in big {
        if rest.peek() == Some(&word) {
            rest.next();
        } else {
            dropped.push(*word);
        }
    }
    (rest.peek().is_none() && !small.is_empty()).then_some(dropped)
}

/// Runs of single letters merged into words: "h e y clanker" -> "hey
/// clanker". None when there is no run of two or more.
fn join_letters(words: &[&str]) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    let mut run = String::new();
    let mut any = false;
    for word in words {
        if word.chars().count() == 1 && word.chars().all(char::is_alphabetic) {
            run.push_str(word);
            continue;
        }
        if run.chars().count() >= 2 {
            any = true;
        }
        if !run.is_empty() {
            out.push(std::mem::take(&mut run));
        }
        out.push(word.to_string());
    }
    if run.chars().count() >= 2 {
        any = true;
    }
    if !run.is_empty() {
        out.push(run);
    }
    any.then(|| out.join(" "))
}

/// Every list in a config: (target, seeds, phrases as written).
pub fn lists(
    config: &toml::Value,
    actions: Option<&toml::Value>,
) -> Vec<(String, Vec<String>, Vec<String>)> {
    let strings = |v: Option<&toml::Value>| -> Vec<String> {
        v.and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let defaults = VoiceCommands::default();
    let seeds = |target: &str| -> Vec<String> {
        match target {
            "enter" => defaults.enter.clone(),
            "sign_off" => defaults.sign_off.clone(),
            "new_line" => defaults.new_line.clone(),
            "undo" => defaults.undo.clone(),
            "back" => defaults.back.clone(),
            "backtrack" => defaults.backtrack.clone(),
            "standby" => defaults.standby.clone(),
            "wake" => defaults.wake.clone(),
            "sleep" => defaults.sleep.clone(),
            "literal" => defaults.literal.clone(),
            _ => Vec::new(),
        }
    };
    let mut out = Vec::new();
    let commands = config.get("commands");
    for target in COMMAND_TARGETS {
        let list = strings(commands.and_then(|c| c.get(*target)));
        if !list.is_empty() {
            out.push((target.to_string(), seeds(target), list));
        }
    }
    if let Some(run) = commands
        .and_then(|c| c.get("run"))
        .and_then(toml::Value::as_table)
    {
        for (name, entry) in run {
            let list = strings(entry.get("phrases"));
            if !list.is_empty() {
                out.push((format!("run:{name}"), Vec::new(), list));
            }
        }
    }
    if let Some(actions) = actions
        .and_then(|a| a.get("action"))
        .and_then(toml::Value::as_array)
    {
        for action in actions {
            let Some(name) = action.get("name").and_then(toml::Value::as_str) else {
                continue;
            };
            let list = strings(action.get("text"));
            if !list.is_empty() {
                out.push((format!("action:{name}"), Vec::new(), list));
            }
        }
    }
    out
}

/// The cs file for a config's lists, and the splits it came from.
pub fn migrate(
    config: &toml::Value,
    actions: Option<&toml::Value>,
    model: &str,
    generated: &str,
) -> (CsFile, Vec<Split>) {
    let splits: Vec<Split> = lists(config, actions)
        .into_iter()
        .map(|(target, seeds, list)| split_list(&target, &seeds, &list))
        .collect();
    let entry = splits
        .iter()
        .flat_map(|s| {
            s.variants.iter().map(|v| Entry {
                meant: v.meant.clone(),
                target: s.target.clone(),
                heard: v.heard.clone(),
                mode: v.mode,
                source: Source::Hand,
                count: 0,
                trials: 0,
                risk: 0.0,
                risk_reason: String::new(),
                status: Status::Active,
            })
        })
        .collect();
    let file = CsFile {
        meta: Meta {
            model: model.to_string(),
            domain: "commands".into(),
            generated: generated.to_string(),
            generator: "ears cs migrate".into(),
        },
        entry,
    };
    (file, splits)
}

/// The cs file as TOML, with a review header: per list, what stays
/// canonical in the config and what moved here.
pub fn render(file: &CsFile, splits: &[Split], from: &str) -> Result<String, toml::ser::Error> {
    let mut out = format!(
        "# Generated by `ears cs migrate` from {from}.\n\
         # Review before use: each entry below is a hand variant in your config\n\
         # read as a mishearing of `meant`. The config is unchanged; once this\n\
         # file is in ~/.config/ears/cs.d/, the variants can be removed there.\n\
         #\n"
    );
    for split in splits.iter().filter(|s| !s.variants.is_empty()) {
        out.push_str(&format!(
            "# {}: keeps {:?}, moves {}\n",
            split.target,
            split.canonical,
            split.variants.len()
        ));
    }
    out.push('\n');
    out.push_str(&file.to_toml()?);
    Ok(out)
}

/// Today as YYYY-MM-DD (UTC).
pub fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    civil(days)
}

/// Days since 1970-01-01 as a date (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `ears cs migrate`: read `config_path` (and `actions_path` if it exists),
/// write the cs file to `out` or stdout. Neither input is changed.
pub fn run(
    config_path: &std::path::Path,
    actions_path: &std::path::Path,
    model: &str,
    out: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let read = |path: &std::path::Path| -> anyhow::Result<toml::Value> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    };
    let config = read(config_path)?;
    let actions = if actions_path.exists() {
        Some(read(actions_path)?)
    } else {
        None
    };
    let (file, splits) = migrate(&config, actions.as_ref(), model, &today());
    let mut from = config_path.display().to_string();
    if actions.is_some() {
        from.push_str(&format!(" and {}", actions_path.display()));
    }
    let text = render(&file, &splits, &from)?;
    match out {
        Some(path) => {
            if path == config_path || path == actions_path {
                anyhow::bail!("--out must not be the file being migrated");
            }
            std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("Wrote {} entries to {}", file.entry.len(), path.display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_770), "2026-11-13");
        assert_eq!(civil(11_016), "2000-02-29");
    }

    use super::*;

    fn s(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    fn variant<'a>(split: &'a Split, heard: &str) -> Option<&'a Variant> {
        split.variants.iter().find(|v| v.heard == heard)
    }

    #[test]
    fn keys_and_distance() {
        assert_eq!(phonetic_key("ears off"), phonetic_key("years off"));
        assert_eq!(phonetic_key("hey clanker"), phonetic_key("hey klanker"));
        assert_eq!(phonetic_key("centurion"), phonetic_key("century on"));
        assert_eq!(phonetic_distance("", ""), 0.0);
        assert!(phonetic_distance("yourself", "ears off") <= NEAR);
        assert!(phonetic_distance("fuck off", "ears off") > NEAR);
        assert!(phonetic_distance("adsense", "ears on") > NEAR);
    }

    #[test]
    fn letters_split_and_truncation() {
        assert_eq!(
            relate("h e y clanker", "hey clanker").map(|r| r.0),
            Some(FailureMode::Spelled)
        );
        assert_eq!(
            relate("ears v a d", "ears vad").map(|r| r.0),
            Some(FailureMode::Spelled)
        );
        assert_eq!(
            relate("back track", "backtrack").map(|r| r.0),
            Some(FailureMode::Split)
        );
        assert_eq!(
            relate("clanker", "hey clanker").map(|r| r.0),
            Some(FailureMode::Truncation)
        );
        assert_eq!(
            relate("f off", "fuck off").map(|r| r.0),
            Some(FailureMode::Truncation)
        );
        // A shorter phrase that drops a real word is its own phrase.
        assert_eq!(relate("cursor", "cursor mode"), None);
        // And a longer one.
        assert_eq!(relate("peace out", "peace"), None);
        assert_eq!(relate("fuck off clanker", "fuck off"), None);
        // Different words that only share a few sounds.
        assert_eq!(relate("stick to centurion", "talk to centurion"), None);
        assert_eq!(relate("ears out", "peace out"), None);
        // Words put in, reworded, or written another way.
        assert_eq!(relate("scroll to the top", "scroll to top"), None);
        assert_eq!(relate("move window left", "move left"), None);
        assert_eq!(relate("make it fullscreen", "toggle fullscreen"), None);
        assert_eq!(relate("make it fullscreen", "full screen"), None);
        assert_eq!(relate("launcher", "launch apps"), None);
        assert_eq!(relate("auto scroll down", "keep scrolling"), None);
        assert_eq!(relate("super control a", "audio controls"), None);
        assert_eq!(relate("send it", "submit"), None);
        assert_eq!(relate("kill window", "close window"), None);
        assert_eq!(relate("super ctrl v", "super control v"), None);
        assert_eq!(relate("tmux window 4", "tmux window four"), None);
        assert_eq!(
            relate("fuck of", "fuck off").map(|r| r.0),
            Some(FailureMode::Homophone)
        );
    }

    #[test]
    fn homophones_and_names() {
        assert_eq!(
            relate("years off", "ears off").map(|r| r.0),
            Some(FailureMode::Homophone)
        );
        assert_eq!(
            relate("hey klinker", "hey clanker").map(|r| r.0),
            Some(FailureMode::NameDrift)
        );
        assert_eq!(
            relate("yourself", "ears off").map(|r| r.0),
            Some(FailureMode::Homophone)
        );
    }

    #[test]
    fn seeds_and_duplicates() {
        let split = split_list(
            "standby",
            &s(&["ears vad", "ears v a d"]),
            &s(&["ears vad", "ears v a d"]),
        );
        assert!(split.canonical.is_empty() && split.variants.is_empty());
    }

    /// The real lists (centurion, 2026-11-13), from the fixture.
    fn fixture() -> (Vec<Split>, CsFile) {
        let config: toml::Value = toml::from_str(include_str!("testdata/config.toml")).unwrap();
        let actions: toml::Value = toml::from_str(include_str!("testdata/actions.toml")).unwrap();
        let (file, splits) = migrate(&config, Some(&actions), "any", "2026-11-13");
        (splits, file)
    }

    fn of<'a>(splits: &'a [Split], target: &str) -> &'a Split {
        splits.iter().find(|s| s.target == target).unwrap()
    }

    #[test]
    fn real_sleep_list() {
        let (splits, _) = fixture();
        let sleep = of(&splits, "sleep");
        assert_eq!(
            sleep.canonical,
            s(&["fuck off", "fuck off clanker"]),
            "{sleep:#?}"
        );
        let yourself = variant(sleep, "yourself").expect("yourself is a variant");
        assert_eq!(yourself.meant, "ears off");
        assert_eq!(variant(sleep, "years off").unwrap().meant, "ears off");
        assert_eq!(variant(sleep, "f off").unwrap().meant, "fuck off");
        assert_eq!(variant(sleep, "fuck of").unwrap().meant, "fuck off");
        for drift in ["fuck off klanker", "fuck off kleiner", "fuck off klinker"] {
            let v = variant(sleep, drift).unwrap();
            assert_eq!(
                (v.meant.as_str(), v.mode),
                ("fuck off clanker", FailureMode::NameDrift)
            );
        }
    }

    #[test]
    fn real_wake_list() {
        let (splits, _) = fixture();
        let wake = of(&splits, "wake");
        assert_eq!(
            wake.canonical,
            s(&["listen clanker", "hey clanker", "adsense"]),
            "{wake:#?}"
        );
        assert_eq!(
            variant(wake, "h e y clanker").unwrap().mode,
            FailureMode::Spelled
        );
        for drift in [
            "hey glinker",
            "hey clinker",
            "hey clunker",
            "hey klinker",
            "klanker",
        ] {
            assert_eq!(
                variant(wake, drift).unwrap().meant,
                "hey clanker",
                "{drift}"
            );
        }
        assert_eq!(variant(wake, "ad sense").unwrap().mode, FailureMode::Split);
    }

    #[test]
    fn real_other_lists() {
        let (splits, file) = fixture();
        let sign_off = of(&splits, "sign_off");
        assert_eq!(sign_off.canonical, s(&["peace", "peace out", "ears out"]));
        assert_eq!(variant(sign_off, "years out").unwrap().meant, "ears out");
        assert_eq!(of(&splits, "enter").canonical, s(&["over"]));
        let centurion = of(&splits, "run:centurion");
        assert_eq!(centurion.canonical, s(&["centurion"]));
        assert_eq!(centurion.variants.len(), 4);
        assert_eq!(of(&splits, "run:cursor").variants, vec![]);
        assert_eq!(
            variant(of(&splits, "action:fullscreen"), "full screen")
                .unwrap()
                .mode,
            FailureMode::Split
        );
        // Every variant became one active hand entry.
        let total: usize = splits.iter().map(|s| s.variants.len()).sum();
        assert_eq!(file.entry.len(), total);
        assert!(file
            .entry
            .iter()
            .all(|e| e.source == Source::Hand && e.status == Status::Active));
    }

    #[test]
    fn rendered_file_reads_back() {
        let (splits, file) = fixture();
        let text = render(&file, &splits, "config.toml").unwrap();
        assert!(text.starts_with("# Generated by `ears cs migrate`"));
        assert_eq!(CsFile::parse(&text).unwrap(), file);
    }
}
