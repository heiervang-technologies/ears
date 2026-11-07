use super::*;
use crate::commands::Command;
use crate::wake::{PhraseWake, Wake, WakeDetector};

const QWEN: &str = "Qwen/Qwen3-ASR-1.7B";

fn loaded(file: &str, toml: &str) -> Loaded {
    Loaded {
        file: file.into(),
        cs: CsFile::parse(toml).unwrap(),
    }
}

fn entry(target: &str, heard: &str) -> String {
    format!("[[entry]]\nmeant = \"x\"\ntarget = \"{target}\"\nheard = \"{heard}\"\n")
}

fn with(entries: &[(&str, &str)]) -> Scaffold {
    let text: String = entries.iter().map(|(t, h)| entry(t, h)).collect();
    Scaffold::merge(Vec::new(), vec![loaded("user.toml", &text)], None)
}

/// The `[commands]` of the real config (fixture), as VAD loads it.
fn real_commands() -> VoiceCommands {
    #[derive(serde::Deserialize)]
    struct File {
        commands: VoiceCommands,
    }
    let file: File = toml::from_str(include_str!("testdata/config.toml")).unwrap();
    file.commands
}

#[test]
fn format_round_trips() {
    let file = CsFile {
        meta: Meta {
            model: "qwen3-asr-1.7b".into(),
            domain: "commands".into(),
            generated: "2026-11-13".into(),
            generator: "ears cs build".into(),
        },
        entry: vec![Entry {
            meant: "ears on".into(),
            target: "wake".into(),
            heard: "adsense".into(),
            mode: FailureMode::Hallucination,
            source: Source::Probe,
            count: 3,
            trials: 24,
            risk: 0.25,
            risk_reason: "rare word".into(),
            status: Status::Flagged,
        }],
    };
    let text = file.to_toml().unwrap();
    assert!(text.contains("[[entry]]") && text.contains("mode = \"hallucination\""));
    assert_eq!(CsFile::parse(&text).unwrap(), file);
    for mode in [
        "homophone",
        "split",
        "spelled",
        "name_drift",
        "truncation",
        "other",
    ] {
        let text =
            format!("[[entry]]\nmeant=\"a\"\ntarget=\"wake\"\nheard=\"b\"\nmode=\"{mode}\"\n");
        assert_eq!(CsFile::parse(&text).unwrap().entry[0].mode.as_str(), mode);
    }
}

#[test]
fn defaults_and_unknown_fields() {
    let file = CsFile::parse(
        "future = 1\n[meta]\nmodel = \"x\"\nnew_meta = true\n\
         [[entry]]\nmeant = \"ears on\"\ntarget = \"wake\"\nheard = \"ears un\"\nconfidence = 0.9\n",
    )
    .unwrap();
    assert_eq!(file.meta.domain, "commands");
    let e = &file.entry[0];
    assert_eq!(
        (e.mode, e.source, e.status, e.count, e.risk),
        (FailureMode::Other, Source::Hand, Status::Active, 0, 0.0)
    );
    assert_eq!(CsFile::parse("").unwrap().meta.model, ANY);
    // Missing required fields do not parse.
    assert!(CsFile::parse("[[entry]]\nmeant = \"a\"\n").is_err());
}

#[test]
fn builtin_files_parse_and_any_ships() {
    assert!(BUILTIN.iter().any(|(name, _)| *name == "any/commands.toml"));
    for (name, text) in BUILTIN {
        let file = CsFile::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let dir = name.split('/').next().unwrap();
        assert_eq!(
            model_key(&file.meta.model),
            dir,
            "{name}: meta.model names its directory"
        );
    }
    assert_eq!(builtin_files().len(), BUILTIN.len());
}

#[test]
fn model_ids_compare_by_last_segment() {
    assert_eq!(model_key(QWEN), "qwen3-asr-1.7b");
    let file = CsFile::parse("[meta]\nmodel = \"qwen3-asr-1.7b\"\n").unwrap();
    assert!(file.applies_to(Some(QWEN)));
    assert!(file.applies_to(Some("qwen3-asr-1.7b")));
    assert!(!file.applies_to(Some("whisper-large-v3")));
    assert!(!file.applies_to(None));
    let any = CsFile::parse("[meta]\nmodel = \"any\"\n").unwrap();
    assert!(any.applies_to(None) && any.applies_to(Some(QWEN)));
}

#[test]
fn files_for_other_models_are_left_out() {
    let qwen = format!(
        "[meta]\nmodel = \"qwen3-asr-1.7b\"\n{}",
        entry("wake", "adsense")
    );
    let whisper = format!(
        "[meta]\nmodel = \"whisper-large-v3\"\n{}",
        entry("wake", "ears own")
    );
    let any = entry("sleep", "years off");
    let files = || vec![loaded("q", &qwen), loaded("w", &whisper), loaded("a", &any)];
    let s = Scaffold::merge(files(), Vec::new(), Some(QWEN));
    assert_eq!(s.model(), Some(QWEN));
    assert_eq!(s.variants("wake"), vec!["adsense"]);
    assert_eq!(s.variants("sleep"), vec!["years off"]);
    let unknown = Scaffold::merge(files(), Vec::new(), None);
    assert!(unknown.variants("wake").is_empty());
    assert_eq!(unknown.variants("sleep"), vec!["years off"]);
}

#[test]
fn user_overrides_builtin_and_rejected_suppresses() {
    let builtin = vec![loaded(
        "builtin:any/commands.toml",
        &(entry("wake", "adsense") + &entry("wake", "ads on") + &entry("sleep", "yourself")),
    )];
    let user = vec![loaded(
        "/u/cs.d/mine.toml",
        "[[entry]]\nmeant = \"ears off\"\ntarget = \"sleep\"\nheard = \"Yourself.\"\nstatus = \"rejected\"\n\
         [[entry]]\nmeant = \"ears on\"\ntarget = \"wake\"\nheard = \"ads on\"\nstatus = \"flagged\"\n\
         [[entry]]\nmeant = \"ears on\"\ntarget = \"wake\"\nheard = \"hey ears\"\n",
    )];
    let s = Scaffold::merge(builtin, user, None);
    assert_eq!(s.variants("wake"), vec!["adsense", "hey ears"]);
    assert!(s.variants("sleep").is_empty());
    // Replaced in place, one entry per (target, heard), from the user file.
    assert_eq!(s.entries().count(), 4);
    let yourself = s.located().find(|l| l.entry.target == "sleep").unwrap();
    assert_eq!(yourself.file, "/u/cs.d/mine.toml");
    assert_eq!(yourself.entry.status, Status::Rejected);
}

#[test]
fn model_files_override_any_files_within_a_layer() {
    let specific = format!(
        "[meta]\nmodel = \"qwen3-asr-1.7b\"\n{}status = \"rejected\"\n",
        entry("wake", "adsense")
    );
    // Given model file first: `any` still goes under it.
    let s = Scaffold::merge(
        vec![
            loaded("q", &specific),
            loaded("a", &entry("wake", "adsense")),
        ],
        Vec::new(),
        Some(QWEN),
    );
    assert!(s.variants("wake").is_empty());
}

#[test]
fn malformed_user_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a-broken.toml"), "[[entry]\nheard = ").unwrap();
    std::fs::write(
        dir.path().join("b-missing-heard.toml"),
        "[[entry]]\nmeant = \"x\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("c-good.toml"), entry("sleep", "years off")).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not toml").unwrap();
    let files = read_dir(dir.path());
    assert_eq!(files.len(), 1);
    assert!(files[0].file.ends_with("c-good.toml"));
    let s = Scaffold::load_from(Some(dir.path()), None);
    assert!(s.variants("sleep").contains(&"years off"));
    assert!(read_dir(&dir.path().join("missing")).is_empty());
}

#[test]
fn empty_heard_or_target_is_skipped() {
    let s = with(&[("wake", " . "), ("", "hello")]);
    assert!(s.is_empty());
}

#[test]
fn active_scaffold_is_empty_until_set() {
    // No test sets it: that would leak into the others.
    assert!(active().is_empty());
}

// --- With no cs entries every list is exactly what it was. ---

#[test]
fn empty_scaffold_leaves_command_lists_identical() {
    let empty = Scaffold::empty();
    for commands in [VoiceCommands::default(), real_commands()] {
        let applied = apply_commands(commands.clone(), &empty);
        assert_eq!(applied, commands);
        assert_eq!(applied.run.resolved(), commands.run.resolved());
        assert!(applied.run.heard_as.is_empty());
    }
    // Entries that do not apply change nothing either.
    let inert = Scaffold::merge(
        vec![loaded(
            "x",
            &format!("[meta]\nmodel = \"other\"\n{}", entry("wake", "adsense")),
        )],
        vec![loaded(
            "y",
            &(entry("sleep", "yourself").replace("heard", "status = \"flagged\"\nheard")),
        )],
        Some(QWEN),
    );
    assert_eq!(apply_commands(real_commands(), &inert), real_commands());
}

#[test]
fn empty_scaffold_leaves_actions_and_decisions_identical() {
    let (_, actions) =
        crate::command_mode::parse_actions(include_str!("testdata/actions.toml")).unwrap();
    let mut applied = actions.clone();
    apply_actions(&mut applied, &Scaffold::empty());
    let texts = |a: &[crate::command_mode::Action]| {
        a.iter()
            .map(|a| (a.name.clone(), a.text.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(texts(&applied), texts(&actions));
    let options = vec!["deploy now".to_string(), "wait".to_string()];
    assert!(decide_variants(&options, &Scaffold::empty()).is_empty());
}

#[test]
fn serialized_config_never_carries_variants() {
    let s = with(&[("run:friend-wake", "hey fiend"), ("wake", "adsense")]);
    let applied = apply_commands(VoiceCommands::default(), &s);
    let text = toml::to_string(&applied).unwrap();
    assert!(!text.contains("fiend"), "{text}");
}

// --- Each target's wiring. ---

#[test]
fn command_lists_take_their_variants() {
    let s = with(&[
        ("enter", "anter"),
        ("sign_off", "over and our"),
        ("new_line", "you line"),
        ("undo", "scratch fat"),
        ("back", "bag"),
        ("backtrack", "back trek"),
        ("literal", "litteral"),
        ("enter", "Enter."),
    ]);
    let commands = apply_commands(
        VoiceCommands {
            enabled: true,
            ..VoiceCommands::default()
        },
        &s,
    );
    // Appended once; one already there (normalized) is not added again.
    assert_eq!(commands.enter, vec!["enter", "anter"]);
    assert_eq!(commands.parse("Anter."), Some(Command::Enter));
    assert_eq!(commands.parse("over and our"), Some(Command::SignOff));
    assert_eq!(commands.parse("You line"), Some(Command::NewLine));
    assert_eq!(commands.parse("scratch fat"), Some(Command::Undo));
    assert_eq!(commands.parse("bag three"), Some(Command::Back(3)));
    assert_eq!(commands.parse("back trek"), Some(Command::Backtrack));
    assert_eq!(
        commands.parse("litteral over"),
        Some(Command::Literal("over".into()))
    );
    // Matching is unchanged: a variant inside a sentence is text.
    assert_eq!(commands.parse("anter the room"), None);
}

#[test]
fn wake_phrases_take_their_variants() {
    let s = with(&[
        ("standby", "ears bad"),
        ("wake", "adsense"),
        ("sleep", "yourself"),
    ]);
    let c = apply_commands(VoiceCommands::default(), &s);
    let mut wake = PhraseWake::new(&c.standby, &c.wake, &c.sleep);
    assert_eq!(wake.check_transcript("Ears bad."), Some(Wake::Toggle));
    assert_eq!(wake.check_transcript("Adsense!"), Some(Wake::On));
    assert_eq!(wake.check_transcript("Yourself."), Some(Wake::Off));
    assert_eq!(wake.check_transcript("Ears on."), Some(Wake::On));
    assert_eq!(wake.check_transcript("do it yourself"), None);
}

#[test]
fn run_commands_take_their_variants() {
    let s = with(&[
        ("run:friend-wake", "hey fiend"),
        ("run:friend-wake", "Hey friend!"),
        ("run:nobody", "ghost words"),
    ]);
    let c = apply_commands(VoiceCommands::default(), &s);
    let friend = c
        .run
        .resolved()
        .into_iter()
        .find(|r| r.name == "friend-wake")
        .unwrap();
    assert_eq!(friend.phrases.last().map(String::as_str), Some("hey fiend"));
    assert_eq!(
        friend
            .phrases
            .iter()
            .filter(|p| normalize(p) == "hey friend")
            .count(),
        1
    );
    match c.parse("Hey fiend, how are you?") {
        Some(Command::Run { name, argv }) => {
            assert_eq!(name, "friend-wake");
            assert_eq!(argv.last().map(String::as_str), Some("how are you?"));
        }
        other => panic!("{other:?}"),
    }
    // A target with no run command adds none.
    assert_eq!(
        c.run.resolved().len(),
        VoiceCommands::default().run.resolved().len()
    );
    // Configured phrases replace built-in ones; variants still join them.
    let mut configured = VoiceCommands::default();
    configured.run.entries.insert(
        "friend-wake".into(),
        crate::commands::RunEntry {
            phrases: Some(vec!["yo friend".into()]),
            ..Default::default()
        },
    );
    let c = apply_commands(configured, &s);
    let friend = c
        .run
        .resolved()
        .into_iter()
        .find(|r| r.name == "friend-wake")
        .unwrap();
    assert_eq!(
        friend.phrases,
        vec!["yo friend", "hey fiend", "Hey friend!"]
    );
}

#[test]
fn action_phrasings_take_their_variants() {
    let (_, mut actions) = crate::command_mode::parse_actions(
        "[[action]]\nname = \"volume-up\"\ntext = [\"volume up\", \"louder\"]\nkeys = \"ctrl+b\"\n",
    )
    .unwrap();
    apply_actions(
        &mut actions,
        &with(&[
            ("action:volume-up", "volume app"),
            ("action:leave", "come and"),
        ]),
    );
    let router = crate::command_mode::Router::new(actions, 2.0, 0.4, 5);
    let by = |words: &str| {
        router
            .by_words(words)
            .map(|i| router.actions[i].name.clone())
    };
    assert_eq!(by("Volume app."), Some("volume-up".into()));
    assert_eq!(by("come and"), Some("leave".into()));
    assert_eq!(by("louder"), Some("volume-up".into()));
    assert_eq!(by("volume apps"), None);
}

#[test]
fn decide_options_take_their_variants() {
    let s = with(&[
        ("decide:Deploy now!", "the ploy now"),
        ("decide:wait", "weight"),
    ]);
    let options = vec![
        "deploy now".to_string(),
        "Wait".to_string(),
        "never".to_string(),
    ];
    let heard_as = decide_variants(&options, &s);
    assert_eq!(
        heard_as,
        vec![("the ploy now".to_string(), 0), ("weight".to_string(), 1)]
    );
    let mut decision = crate::decide::Decision {
        id: "t".into(),
        question: "?".into(),
        options,
        pid: 1,
        opened_ms: None,
        timeout_ms: None,
        other: false,
        exact: false,
        heard_as,
    };
    use crate::decide::Heard;
    assert_eq!(decision.hear("The ploy now."), Heard::Chose(0));
    assert_eq!(decision.hear("weight"), Heard::Chose(1));
    assert_eq!(decision.hear("never"), Heard::Chose(2));
    // Exact decisions take an option only as written.
    decision.exact = true;
    assert_eq!(decision.hear("weight"), Heard::Unclear);
    // Not on the wire.
    let json = serde_json::to_string(&decision).unwrap();
    assert!(!json.contains("weight"));
}
