//! `ears cs check`: every entry that applies to the ASR model, with its risk;
//! flagged ones marked. Exits non-zero when an active entry would now be
//! rejected.

use super::risk::{Lexicon, Phrase, Risk, Verdict};
use super::{assess_entry, Located, Scaffold, Status};
use crate::commands::VoiceCommands;

/// A phrase some list matches, and that list (an owned [`Phrase`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPhrase {
    pub text: String,
    pub target: String,
}

/// Every phrase the configured lists match, by target: the `[commands]`
/// lists, `keys`, run commands, command-mode actions (name and phrasings)
/// and the words that enter command mode.
pub fn configured_phrases(
    commands: &VoiceCommands,
    actions: &[crate::command_mode::Action],
) -> Vec<ListPhrase> {
    let mut out = Vec::new();
    let mut add = |target: &str, list: &[String]| {
        out.extend(list.iter().map(|text| ListPhrase {
            text: text.clone(),
            target: target.to_string(),
        }));
    };
    let c = commands;
    for (target, list) in [
        ("enter", &c.enter),
        ("sign_off", &c.sign_off),
        ("new_line", &c.new_line),
        ("undo", &c.undo),
        ("backtrack", &c.backtrack),
        ("standby", &c.standby),
        ("wake", &c.wake),
        ("sleep", &c.sleep),
    ] {
        add(target, list);
    }
    // `back` and `literal` are prefixes, never a whole utterance alone.
    add("keys", &c.keys.keys().cloned().collect::<Vec<_>>());
    for run in c.run.resolved() {
        add(&format!("run:{}", run.name), &run.phrases);
    }
    for action in actions {
        let target = format!("action:{}", action.name);
        add(&target, std::slice::from_ref(&action.name));
        add(&target, &action.text);
    }
    let enter: Vec<String> = crate::command_mode::ENTER_PHRASES
        .iter()
        .map(|s| s.to_string())
        .collect();
    add("command", &enter);
    out
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub located: Located,
    pub risk: Risk,
}

impl Row {
    /// Shown for review: flagged, or risky now.
    pub fn flagged(&self) -> bool {
        self.located.entry.status == Status::Flagged || self.risk.verdict != Verdict::Ok
    }

    /// Would be rejected while applied.
    pub fn failing(&self) -> bool {
        self.located.entry.status == Status::Active && self.risk.verdict == Verdict::Reject
    }
}

/// Assess every entry ([`assess_entry`]) against the configured phrases
/// and the other active entries.
pub fn rows(scaffold: &Scaffold, configured: &[ListPhrase]) -> Vec<Row> {
    let lexicon = Lexicon::builtin();
    let others: Vec<Phrase<'_>> = configured
        .iter()
        .map(|p| Phrase {
            text: &p.text,
            target: &p.target,
        })
        .chain(
            scaffold
                .entries()
                .filter(|e| e.status == Status::Active)
                .map(|e| Phrase {
                    text: &e.heard,
                    target: &e.target,
                }),
        )
        .collect();
    scaffold
        .located()
        .map(|located| Row {
            risk: assess_entry(
                &located.entry.heard,
                &located.entry.meant,
                &located.entry.target,
                &others,
                &lexicon,
            ),
            located: located.clone(),
        })
        .collect()
}

/// The report as a table; `color` marks flagged rows with ANSI colour too.
pub fn render(rows: &[Row], color: bool) -> String {
    const HEAD: [&str; 9] = [
        "FILE", "TARGET", "MEANT", "HEARD", "MODE", "SOURCE", "RISK", "STATUS", "REASON",
    ];
    let cells: Vec<[String; 9]> = rows
        .iter()
        .map(|r| {
            let e = &r.located.entry;
            [
                short_file(&r.located.file),
                e.target.clone(),
                e.meant.clone(),
                e.heard.clone(),
                e.mode.as_str().into(),
                e.source.as_str().into(),
                format!("{:.2}", r.risk.score),
                e.status.as_str().into(),
                r.risk.reason.clone(),
            ]
        })
        .collect();
    let mut width = HEAD.map(str::len);
    for row in &cells {
        for (w, cell) in width.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let line = |mark: &str, row: &[String]| {
        let mut text = mark.to_string();
        for (i, cell) in row.iter().enumerate() {
            if i + 1 == row.len() {
                text.push_str(cell);
            } else {
                text.push_str(&format!("{cell:<w$}  ", w = width[i]));
            }
        }
        text.trim_end().to_string()
    };
    let mut out = line("  ", &HEAD.map(String::from));
    out.push('\n');
    for (row, cell) in rows.iter().zip(&cells) {
        let mark = if row.failing() {
            "x "
        } else if row.flagged() {
            "! "
        } else {
            "  "
        };
        let text = line(mark, cell);
        let paint = match (color, row.failing(), row.flagged()) {
            (true, true, _) => Some("31"),
            (true, false, true) => Some("33"),
            _ => None,
        };
        match paint {
            Some(code) => out.push_str(&format!("\x1b[{code}m{text}\x1b[0m\n")),
            None => out.push_str(&format!("{text}\n")),
        }
    }
    let failing = rows.iter().filter(|r| r.failing()).count();
    let flagged = rows.iter().filter(|r| r.flagged()).count();
    out.push_str(&format!(
        "{} entries, {} flagged, {} rejected while active\n",
        rows.len(),
        flagged,
        failing
    ));
    out
}

/// A built-in file by its repo path, one in cs.d as `cs.d/<name>`, any
/// other by its path.
fn short_file(file: &str) -> String {
    let path = std::path::Path::new(file);
    let in_cs_d = path
        .parent()
        .is_some_and(|dir| dir.file_name().is_some_and(|n| n == "cs.d"));
    match path.file_name() {
        Some(name) if in_cs_d => format!("cs.d/{}", name.to_string_lossy()),
        _ => file.to_string(),
    }
}

/// Command-mode actions as configured, read without writing the default
/// file when there is none.
pub fn read_actions(path: &std::path::Path) -> Vec<crate::command_mode::Action> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    crate::command_mode::parse_actions(&text)
        .or_else(|_| crate::command_mode::parse_actions(""))
        .map(|(_, actions)| actions)
        .unwrap_or_default()
}

/// `ears cs check`. Returns the exit code: 1 when an active entry would be
/// rejected.
pub async fn run(
    config: &crate::Config,
    model: Option<String>,
    files: &[std::path::PathBuf],
) -> anyhow::Result<i32> {
    use std::io::IsTerminal;
    let (model, from) = match model {
        Some(model) => (Some(model), "--model".to_string()),
        None => resolve_model(config).await,
    };
    // Files under review go over the installed ones, as if in cs.d last.
    let mut user = super::user_dir()
        .map(|d| super::read_dir(&d))
        .unwrap_or_default();
    for path in files {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let cs = super::CsFile::parse(&text)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        user.push(super::Loaded {
            file: path.display().to_string(),
            cs,
        });
    }
    let scaffold = Scaffold::merge(super::builtin_files(), user, model.as_deref());
    println!(
        "Model: {} ({from}); user files: {}",
        model
            .as_deref()
            .unwrap_or("unknown, only `any` files apply"),
        super::user_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "none".into())
    );
    let paths = crate::command_mode::Paths::new(&config.state_dir);
    let configured = configured_phrases(&config.commands, &read_actions(&paths.actions));
    let rows = rows(&scaffold, &configured);
    print!("{}", render(&rows, std::io::stdout().is_terminal()));
    Ok(i32::from(rows.iter().any(Row::failing)))
}

/// The ASR model as VAD resolves it: the configured one, else the first the
/// server lists.
async fn resolve_model(config: &crate::Config) -> (Option<String>, String) {
    let language =
        crate::desktop::KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server, model) = config.resolve_server(language.as_deref());
    if let Some(model) = model {
        return (Some(model), "configured".into());
    }
    let client = crate::WhisperClient::new(server.to_string()).with_api_key(config.api_key.clone());
    match client.server_model().await {
        Some(model) => (Some(model), format!("from {server}v1/models")),
        None => (None, format!("{server} did not say")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cs::{CsFile, Loaded};

    fn scaffold(toml: &str) -> Scaffold {
        Scaffold::merge(
            Vec::new(),
            vec![Loaded {
                file: "/home/x/.config/ears/cs.d/mine.toml".into(),
                cs: CsFile::parse(toml).unwrap(),
            }],
            None,
        )
    }

    const FILE: &str = r#"
[[entry]]
meant = "ears off"
target = "sleep"
heard = "yourself"

[[entry]]
meant = "ears off"
target = "sleep"
heard = "Over"

[[entry]]
meant = "ears on"
target = "wake"
heard = "ears of"
status = "flagged"
risk = 0.4
risk_reason = "near ears off"
"#;

    #[test]
    fn exact_collision_with_another_list_is_rejected() {
        let commands = VoiceCommands {
            enter: vec!["enter".into(), "over".into()],
            ..VoiceCommands::default()
        };
        let configured = configured_phrases(&commands, &[]);
        let rows = rows(&scaffold(FILE), &configured);
        assert_eq!(rows.len(), 3);
        // "yourself" is ordinary speech: flagged for review, not rejected.
        assert_eq!(rows[0].risk.verdict, Verdict::Flag, "{:?}", rows[0]);
        assert!(rows[0].flagged() && !rows[0].failing());
        assert!(rows[1].failing(), "{:?}", rows[1]);
        assert!(rows[1].risk.reason.contains("enter"), "{:?}", rows[1]);
        // Flagged in the file: shown, not failing (its sound is near "ears off").
        assert!(rows[2].flagged() && !rows[2].failing(), "{:?}", rows[2]);

        let text = render(&rows, false);
        assert!(text.contains("x cs.d/mine.toml"), "{text}");
        assert!(text.contains("! cs.d/mine.toml"), "{text}");
        assert!(text.contains("yourself"));
        assert!(text.ends_with("3 entries, 3 flagged, 1 rejected while active\n"));
        assert!(!text.contains('\x1b'));
        assert!(render(&rows, true).contains("\x1b[31m"));
    }

    #[test]
    fn another_target_saying_the_meant_words_is_no_collision() {
        // "ears on" both wakes and runs `regular`: "years on" heard for the
        // wake phrase is no new ambiguity...
        let commands: VoiceCommands = toml::from_str(
            r#"
            [run.regular]
            phrases = ["ears on", "regular mode"]
            command = ["haios-voice-route", "cursor"]
            resume = true
            "#,
        )
        .unwrap();
        let configured = configured_phrases(&commands, &[]);
        let entry = |meant: &str, target: &str| {
            format!("[[entry]]\nmeant = {meant:?}\ntarget = {target:?}\nheard = \"years on\"\n")
        };
        let wake = rows(&scaffold(&entry("ears on", "wake")), &configured);
        assert_ne!(wake[0].risk.verdict, Verdict::Reject, "{:?}", wake[0]);
        assert!(!wake[0].failing());
        // ...but meant as "ears off" it still sounds exactly like a wake phrase.
        let sleep = rows(&scaffold(&entry("ears off", "sleep")), &configured);
        assert!(sleep[0].failing(), "{:?}", sleep[0]);
    }

    #[test]
    fn same_list_is_no_collision() {
        let configured = configured_phrases(&VoiceCommands::default(), &[]);
        let s =
            scaffold("[[entry]]\nmeant = \"ears off\"\ntarget = \"sleep\"\nheard = \"ears off\"\n");
        assert!(!rows(&s, &configured)[0].flagged());
    }

    #[test]
    fn other_entries_collide_too() {
        let s = scaffold(
            "[[entry]]\nmeant=\"ears on\"\ntarget=\"wake\"\nheard=\"ears of\"\n\
             [[entry]]\nmeant=\"ears off\"\ntarget=\"sleep\"\nheard=\"ears of\"\n",
        );
        let rows = rows(&s, &[]);
        assert!(rows.iter().all(Row::failing));
    }

    #[test]
    fn configured_phrases_cover_every_kind() {
        let (_, actions) = crate::command_mode::parse_actions("").unwrap();
        let phrases = configured_phrases(&VoiceCommands::default(), &actions);
        let has =
            |target: &str, text: &str| phrases.iter().any(|p| p.target == target && p.text == text);
        assert!(has("wake", "ears on"));
        assert!(has("sleep", "ears off"));
        assert!(has("run:friend-wake", "hey friend"));
        assert!(has("action:leave", "exit command mode"));
        assert!(has("action:wrong", "wrong"));
        assert!(has("command", "command"));
    }
}
