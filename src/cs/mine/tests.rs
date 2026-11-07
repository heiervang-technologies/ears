use super::*;
use crate::command_mode::{Effect, Hearing, Outcome};
use std::os::unix::fs::PermissionsExt as _;
use std::time::Instant;

fn commands() -> VoiceCommands {
    let mut c: VoiceCommands = toml::from_str(
        r#"
        enabled = true
        enter = ["over"]
        sleep = ["years off"]
        wake = ["hey clanker"]
        [keys]
        "close tab" = "ctrl+w"
        "#,
    )
    .unwrap();
    c.enabled = true;
    c
}

fn ctx(commands: &VoiceCommands, mode: ModeKind) -> Context<'_> {
    Context {
        commands,
        mode,
        command_mode: true,
        vision: false,
        guided: false,
        max_distance: CsConfig::default().mine_max_distance,
    }
}

fn line_at(time: f64, mode: &str, text: &str, matched: Option<(&str, &str)>) -> Line {
    let mut line = Line::new(mode, text.to_string());
    line.time = time;
    if let Some((target, phrase)) = matched {
        line = line.matched(target, phrase);
    }
    line
}

fn params(min_count: u32) -> MineParams {
    MineParams {
        window_s: 6.0,
        min_count,
        max_distance: CsConfig::default().mine_max_distance,
        since: None,
    }
}

// --- configuration -------------------------------------------------------

#[test]
fn old_configs_load_and_saving_keeps_the_section() {
    let old: crate::config::Config = toml::from_str("device = \"mic\"\n").unwrap();
    assert!(!old.cs.journal);
    assert_eq!(old.cs, CsConfig::default());
    let saved = toml::to_string_pretty(&old).unwrap();
    assert!(!saved.contains("[cs]"), "{saved}");

    let on: crate::config::Config =
        toml::from_str("device = \"mic\"\n[cs]\njournal = true\njournal_max_days = 7\n").unwrap();
    assert!(on.cs.journal);
    assert_eq!(on.cs.journal_max_days, 7);
    assert_eq!(on.cs.journal_max_kb, CsConfig::default().journal_max_kb);
    let again: crate::config::Config =
        toml::from_str(&toml::to_string_pretty(&on).unwrap()).unwrap();
    assert_eq!(again.cs, on.cs);
}

#[test]
fn the_journal_is_off_by_default() {
    assert!(Journal::from_config(&CsConfig::default()).is_none());
}

#[test]
fn journal_path_follows_xdg_state_home() {
    // Read only: the variable is not changed, the result is checked
    // against whatever it holds.
    let path = journal_path().unwrap();
    assert!(path.ends_with("ears/cs-journal.jsonl"));
    match std::env::var_os("XDG_STATE_HOME").filter(|p| Path::new(p).is_absolute()) {
        Some(base) => assert!(path.starts_with(base)),
        None => assert!(path.to_string_lossy().contains(".local/state")),
    }
}

// --- what is journaled (privacy) -------------------------------------------

#[test]
fn ordinary_dictation_is_never_journaled() {
    let c = commands();
    for text in [
        "dear doctor, my results came back",
        "the password is hunter two",
        "literal over",
        "close the tab please",
    ] {
        assert_eq!(
            transcript_line(&ctx(&c, ModeKind::Dictation), text),
            None,
            "{text}"
        );
        assert_eq!(
            transcript_line(&ctx(&c, ModeKind::Vision), text),
            None,
            "{text}"
        );
    }
}

#[test]
fn dictation_commands_are_journaled_without_the_dictated_words() {
    let c = commands();
    let d = |text| transcript_line(&ctx(&c, ModeKind::Dictation), text).unwrap();

    let sleep = d("Years off.");
    assert_eq!(sleep.mode, "dictation");
    assert_eq!(sleep.text, "years off");
    assert_eq!(sleep.target.as_deref(), Some("sleep"));
    assert_eq!(sleep.phrase.as_deref(), Some("years off"));

    let enter = d("over");
    assert_eq!(enter.target.as_deref(), Some("enter"));

    let keys = d("close tab");
    assert_eq!(keys.target.as_deref(), Some("keys"));

    // A command closing a dictated sentence: the command's words only.
    let tail = d("tell my sister I love her over");
    assert_eq!(tail.text, "over");
    assert!(tail.tail);
    assert!(!serde_json::to_string(&tail).unwrap().contains("sister"));

    // A prefix run command: its phrase only, not what was passed on.
    let run = d("hey friend what is my bank balance");
    assert_eq!(run.text, "hey friend");
    assert_eq!(run.target.as_deref(), Some("run:friend-wake"));
    assert!(!serde_json::to_string(&run).unwrap().contains("bank"));

    let command = d("command");
    assert_eq!(command.target.as_deref(), Some("command"));
}

#[test]
fn standby_keeps_unmatched_utterances() {
    let c = commands();
    let s = |text| transcript_line(&ctx(&c, ModeKind::Standby), text).unwrap();
    let missed = s("hay clanker");
    assert_eq!(missed.mode, "standby");
    assert!(!missed.is_matched());
    let wake = s("Ears on!");
    assert_eq!(wake.target.as_deref(), Some("wake"));
    assert_eq!(wake.phrase.as_deref(), Some("ears on"));
    let colour = s("orange");
    assert_eq!(colour.target.as_deref(), Some("colour:orange"));
}

#[test]
fn standby_drops_speech_that_sounds_like_no_standby_phrase() {
    let c = commands();
    let s = |text| transcript_line(&ctx(&c, ModeKind::Standby), text);
    // Overheard or private: nowhere near a phrase standby listens for.
    for text in [
        "what time is it",
        "call mom",
        "i love you",
        "pass me the salt",
        "dinner is ready",
        "my bank pin is four four two one",
    ] {
        assert_eq!(s(text), None, "{text}");
    }
    // Near misses of standby phrases are kept, unmatched.
    for (text, near) in [
        ("hay glinker", "hey clanker"),
        ("ears of", "ears off"),
        ("hears on", "ears on"),
        ("hi friends", "hey friend"),
        ("commend", "command"),
    ] {
        let line = s(text).unwrap_or_else(|| panic!("{text} near {near}"));
        assert!(!line.is_matched(), "{text}");
    }
    // The limit is the configured one.
    let mut strict = ctx(&c, ModeKind::Standby);
    strict.max_distance = 0.0;
    assert_eq!(transcript_line(&strict, "ears of"), None);
    assert!(nearest_standby_phrase(&strict, "ears off") == 0.0);
}

#[test]
fn guided_grammar_and_empty_text_are_not_journaled() {
    let c = commands();
    let mut guided = ctx(&c, ModeKind::Standby);
    guided.guided = true;
    assert_eq!(transcript_line(&guided, "ls -la"), None);
    assert_eq!(transcript_line(&ctx(&c, ModeKind::Standby), " . "), None);
}

#[test]
fn command_mode_lines_carry_the_routing() {
    let fire = |_by: &'static str| Outcome::Fire {
        name: "close-window".into(),
        effect: Effect::Nothing,
    };
    let hearing = |words: &str, by: &'static str| Hearing {
        words: words.into(),
        by: Some(by),
        ..Default::default()
    };
    let words = command_line(&fire("words"), &hearing("Close window.", "words")).unwrap();
    assert_eq!(words.mode, "command");
    assert_eq!(words.target.as_deref(), Some("action:close-window"));
    assert_eq!(words.phrase.as_deref(), Some("close window"));
    assert_eq!(words.via, None);
    assert_eq!(words.routing.as_ref().unwrap()["by"], "words");

    let voice = command_line(&fire("voice"), &hearing("clothes window", "voice")).unwrap();
    assert_eq!(voice.text, "clothes window");
    assert_eq!(voice.phrase.as_deref(), Some("close window"));
    assert_eq!(voice.via.as_deref(), Some("voice"));

    let missed = command_line(
        &Outcome::Nothing,
        &Hearing {
            words: "clothes".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!missed.is_matched());
    assert!(missed.routing.is_some());

    assert!(command_line(&Outcome::Taught("x".into()), &hearing("x", "words")).is_none());
    assert!(command_line(&Outcome::Nothing, &Hearing::default()).is_none());
}

#[test]
fn decision_lines_skip_answers_of_your_own() {
    let options = vec!["Ship it".to_string(), "Wait".to_string()];
    let chose = decision_line(&options, Some(0), "chose", "ship it", "words").unwrap();
    assert_eq!(chose.mode, "decide");
    assert_eq!(chose.target.as_deref(), Some("decide:ship it"));
    let by_sound = decision_line(&options, Some(1), "chose", "weight", "embedding").unwrap();
    assert_eq!(by_sound.via.as_deref(), Some("embedding"));
    let unclear = decision_line(&options, None, "unclear", "chip it", "words").unwrap();
    assert!(!unclear.is_matched());
    assert!(decision_line(&options, None, "custom", "call my mother first", "words").is_none());
}

// --- the writer ----------------------------------------------------------

#[test]
fn a_full_queue_drops_lines_without_blocking() {
    let (tx, rx) = sync_channel(2);
    let journal = Journal::with_sender(tx);
    let started = Instant::now();
    for i in 0..1000 {
        journal.send(line_at(i as f64, "standby", "x", None));
    }
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(journal.dropped(), 998);
    assert_eq!(rx.try_iter().count(), 2);
    // A writer that has gone away does not block or panic either.
    drop(rx);
    journal.send(line_at(0.0, "standby", "x", None));
    assert_eq!(journal.dropped(), 999);
}

#[test]
fn the_writer_appends_private_lines_with_the_drop_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/ears").join(JOURNAL);
    let config = CsConfig {
        journal: true,
        ..Default::default()
    };
    let journal = Journal::start(&path, &config).unwrap();
    journal.dropped.store(3, Ordering::Relaxed);
    let mut line = Line::new("standby", "hey glinker".into());
    line.time = now();
    journal.send(line);
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_journal(&path).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let lines = read_journal(&path);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "hey glinker");
    assert_eq!(lines[0].dropped, Some(3));
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_writer_rotates_past_the_size_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(JOURNAL);
    let config = CsConfig {
        journal_max_kb: 1,
        ..Default::default()
    };
    let mut writer = Writer::new(&path, &config, Arc::new(AtomicU64::new(0)));
    writer.last_prune = f64::MAX / 2.0;
    let t = now();
    for i in 0..60 {
        writer
            .append(line_at(
                t + i as f64,
                "standby",
                "some words heard here",
                None,
            ))
            .unwrap();
    }
    let current = std::fs::metadata(&path).unwrap().len();
    let old = std::fs::metadata(rotated(&path)).unwrap();
    assert!(current <= 1024, "{current}");
    assert!(old.len() > 1024);
    assert_eq!(old.permissions().mode() & 0o777, 0o600);
    // Only one old file: all that is kept is under twice the cap.
    assert!(current + old.len() < 2 * 1024 + 200);
    let lines = read_journal(&path);
    assert!(!lines.is_empty());
    assert!(lines.windows(2).all(|w| w[0].time <= w[1].time));
}

#[test]
fn the_writer_drops_lines_past_the_age_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(JOURNAL);
    let config = CsConfig {
        journal_max_days: 2,
        ..Default::default()
    };
    let mut writer = Writer::new(&path, &config, Arc::new(AtomicU64::new(0)));
    let t = now();
    writer.last_prune = t;
    writer
        .append(line_at(t - 3.0 * 86400.0, "standby", "old", None))
        .unwrap();
    writer
        .append(line_at(t - 86400.0, "standby", "recent", None))
        .unwrap();
    std::fs::write(rotated(&path), "{}\n").unwrap();
    writer.prune(t);
    let texts: Vec<String> = read_journal(&path).into_iter().map(|l| l.text).collect();
    assert_eq!(texts, ["recent"]);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // The rotated file is newer than the cap: kept, until it is not.
    assert!(rotated(&path).exists());
    writer.prune(t + 3.0 * 86400.0);
    assert!(!rotated(&path).exists());
    assert!(read_journal(&path).is_empty());
}

// --- the miner -----------------------------------------------------------

#[test]
fn a_miss_then_a_match_proposes_a_variant() {
    let lines = vec![
        line_at(100.0, "standby", "hey glinker", None),
        line_at(
            102.0,
            "standby",
            "hey clanker",
            Some(("wake", "hey clanker")),
        ),
        line_at(500.0, "standby", "hey glinker", None),
        line_at(501.0, "standby", "hey glinker", None),
        line_at(
            503.5,
            "standby",
            "hey clanker",
            Some(("wake", "hey clanker")),
        ),
    ];
    let proposals = mine(&lines, &params(2));
    assert_eq!(proposals.len(), 1);
    let p = &proposals[0];
    assert_eq!(
        (p.target.as_str(), p.heard.as_str(), p.meant.as_str()),
        ("wake", "hey glinker", "hey clanker")
    );
    assert_eq!(p.count, 3);
    assert_eq!(p.evidence.get("retry"), Some(&3));
}

#[test]
fn unrelated_utterances_seconds_apart_do_not_pair() {
    let lines = vec![
        line_at(100.0, "standby", "what time is it", None),
        line_at(102.0, "standby", "ears off", Some(("sleep", "ears off"))),
        line_at(200.0, "standby", "pass me the salt", None),
        line_at(201.0, "standby", "ears on", Some(("wake", "ears on"))),
    ];
    assert!(mine(&lines, &params(1)).is_empty());
}

#[test]
fn pairs_need_the_window_the_same_mode_and_no_match_between() {
    let mine1 = |lines: &[Line]| mine(lines, &params(1));
    // Too late.
    assert!(mine1(&[
        line_at(0.0, "standby", "years on", None),
        line_at(10.0, "standby", "ears on", Some(("wake", "ears on"))),
    ])
    .is_empty());
    // Another mode.
    assert!(mine1(&[
        line_at(0.0, "standby", "years on", None),
        line_at(1.0, "dictation", "ears on", Some(("wake", "ears on"))),
    ])
    .is_empty());
    // Paired with the first match after it only.
    let p = mine1(&[
        line_at(0.0, "command", "clothes", None),
        line_at(1.0, "command", "close", Some(("action:close", "close"))),
        line_at(2.0, "command", "lose", Some(("action:lose", "lose"))),
    ]);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].target, "action:close");
    // Colour words are not proposed for.
    assert!(mine1(&[
        line_at(0.0, "standby", "orinj", None),
        line_at(1.0, "standby", "orange", Some(("colour:orange", "orange"))),
    ])
    .is_empty());
}

#[test]
fn a_single_sighting_is_below_the_minimum() {
    let lines = vec![
        line_at(0.0, "standby", "years off", None),
        line_at(1.0, "standby", "ears off", Some(("sleep", "ears off"))),
    ];
    assert!(mine(&lines, &params(2)).is_empty());
    assert_eq!(mine(&lines, &params(1)).len(), 1);
}

#[test]
fn since_limits_the_lines_mined() {
    let lines = vec![
        line_at(0.0, "standby", "years off", None),
        line_at(1.0, "standby", "ears off", Some(("sleep", "ears off"))),
        line_at(100.0, "standby", "years off", None),
        line_at(101.0, "standby", "ears off", Some(("sleep", "ears off"))),
    ];
    let mut p = params(1);
    p.since = Some(50.0);
    assert_eq!(mine(&lines, &p)[0].count, 1);
}

#[test]
fn routed_fires_and_chant_readings_are_evidence() {
    let mut routed = line_at(
        0.0,
        "command",
        "clothes window",
        Some(("action:close-window", "close window")),
    );
    routed.via = Some("voice".into());
    let mut chant = line_at(
        10.0,
        "command",
        "close window",
        Some(("action:close-window", "close window")),
    );
    chant.alts = vec!["clothes window".into(), "close window".into()];
    let p = mine(&[routed, chant], &params(2));
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].heard, "clothes window");
    assert_eq!(p[0].evidence.get("routed"), Some(&1));
    assert_eq!(p[0].evidence.get("chant"), Some(&1));

    // Fired by meaning from different-sounding words: not a mishearing.
    let mut meaning = line_at(
        0.0,
        "command",
        "shut it",
        Some(("action:close-window", "close window")),
    );
    meaning.via = Some("meaning".into());
    assert!(mine(&[meaning], &params(1)).is_empty());
}

#[test]
fn failure_modes() {
    assert_eq!(classify("years off", "ears off"), FailureMode::Homophone);
    assert_eq!(
        classify("hey glinker", "hey clanker"),
        FailureMode::NameDrift
    );
    assert_eq!(classify("back track", "backtrack"), FailureMode::Split);
    assert_eq!(classify("ears v a d", "ears vad"), FailureMode::Spelled);
    assert_eq!(classify("ears", "ears on"), FailureMode::Truncation);
    assert_eq!(classify("adsense", "ears on"), FailureMode::Hallucination);
}

#[test]
fn entries_are_always_flagged_and_mined() {
    let proposals = vec![
        Proposal {
            target: "sleep".into(),
            heard: "years off".into(),
            meant: "ears off".into(),
            count: 4,
            evidence: [("retry", 4)].into_iter().collect(),
        },
        Proposal {
            target: "sleep".into(),
            heard: "ears on".into(),
            meant: "ears off".into(),
            count: 2,
            evidence: [("retry", 2)].into_iter().collect(),
        },
    ];
    let known = known_phrases(&commands());
    let e = entries(&proposals, &known);
    assert!(e
        .iter()
        .all(|e| e.status == Status::Flagged && e.source == Source::Mined));
    assert_eq!(e[0].count, 4);
    assert_eq!(e[0].mode, FailureMode::Homophone);
    assert!(e[0].risk < 1.0);
    assert!(e[0].risk_reason.contains("retry 4"));
    // "ears on" is the wake phrase: an exact collision.
    assert_eq!(e[1].risk, 1.0);
    assert!(e[1].risk_reason.contains("wake"), "{}", e[1].risk_reason);
}

fn entry(target: &str, heard: &str, count: u32, status: Status) -> Entry {
    Entry {
        meant: "ears off".into(),
        target: target.into(),
        heard: heard.into(),
        mode: FailureMode::Homophone,
        source: Source::Mined,
        count,
        trials: 0,
        risk: 0.0,
        risk_reason: String::new(),
        status,
    }
}

#[test]
fn merging_keeps_reviewed_entries() {
    let existing = vec![
        entry("sleep", "yourself", 9, Status::Rejected),
        entry("sleep", "years off", 2, Status::Flagged),
        entry("sleep", "ears of", 3, Status::Active),
        entry("sleep", "old guess", 2, Status::Flagged),
    ];
    let fresh = vec![
        entry("sleep", "yourself", 12, Status::Flagged),
        entry("sleep", "years off", 5, Status::Flagged),
        entry("sleep", "ears of", 7, Status::Flagged),
        entry("wake", "hears on", 2, Status::Flagged),
    ];
    let merged = merge(existing, fresh);
    let get = |h: &str| merged.iter().find(|e| e.heard == h).unwrap();
    assert_eq!(merged.len(), 5);
    assert_eq!(get("yourself").status, Status::Rejected);
    assert_eq!(get("yourself").count, 9);
    assert_eq!(get("ears of").status, Status::Active);
    assert_eq!(get("years off").count, 5);
    assert_eq!(get("old guess").count, 2);
    assert_eq!(get("hears on").status, Status::Flagged);
}

#[test]
fn the_review_file_reads_back() {
    let text = render("any", vec![entry("sleep", "years off", 2, Status::Flagged)]).unwrap();
    assert!(text.starts_with("# Mined by"));
    let file: CsFile = toml::from_str(&text).unwrap();
    assert_eq!(file.meta.generator, "ears cs mine");
    assert_eq!(file.meta.domain, "commands");
    assert_eq!(file.meta.generated.len(), 10);
    assert_eq!(file.entry.len(), 1);
    assert!(text.contains("status = \"flagged\""));
    assert!(text.contains("source = \"mined\""));
}

#[test]
fn run_writes_a_review_file_from_a_journal() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join(JOURNAL);
    let out = dir.path().join("cs.d/mined.toml");
    let t = now() - 60.0;
    let lines = [
        line_at(t, "standby", "years off", None),
        line_at(t + 1.0, "standby", "ears off", Some(("sleep", "ears off"))),
        line_at(t + 30.0, "standby", "years off", None),
        line_at(t + 31.0, "standby", "ears off", Some(("sleep", "ears off"))),
    ];
    let text: String = lines
        .iter()
        .map(|l| serde_json::to_string(l).unwrap() + "\n")
        .collect();
    std::fs::write(&journal, text + "not json\n").unwrap();
    let mut config: crate::config::Config = toml::from_str("").unwrap();
    config.config_dir = dir.path().join("config");
    let args = MineArgs {
        journal: Some(journal.clone()),
        out: Some(out.clone()),
        ..Default::default()
    };
    run(&config, &args).unwrap();
    let file: CsFile = toml::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(file.entry.len(), 1);
    assert_eq!(file.entry[0].heard, "years off");
    assert_eq!(file.entry[0].meant, "ears off");
    assert_eq!(file.entry[0].count, 2);
    assert_eq!(file.entry[0].status, Status::Flagged);

    // A reviewer rejects it; mining again keeps that.
    let reviewed = std::fs::read_to_string(&out)
        .unwrap()
        .replace("status = \"flagged\"", "status = \"rejected\"");
    std::fs::write(&out, reviewed).unwrap();
    run(&config, &args).unwrap();
    let file: CsFile = toml::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(file.entry.len(), 1);
    assert_eq!(file.entry[0].status, Status::Rejected);

    // --since past every line: nothing new, the file stays as reviewed.
    let recent = MineArgs {
        since: Some(Duration::from_secs(1)),
        ..args.clone()
    };
    run(&config, &recent).unwrap();
    let file: CsFile = toml::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(file.entry[0].status, Status::Rejected);
}

#[test]
fn durations() {
    assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
    assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
    assert_eq!(parse_duration("12h").unwrap(), Duration::from_secs(43200));
    assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(604800));
    assert_eq!(parse_duration("2w").unwrap(), Duration::from_secs(1209600));
    assert!(parse_duration("7y").is_err());
    assert!(parse_duration("d").is_err());
}

#[test]
fn today_is_a_date() {
    let d = today();
    assert_eq!(d.len(), 10);
    assert!(d.starts_with("20"));
    assert_eq!(&d[4..5], "-");
}

fn loaded(file: &str, entries: Vec<Entry>, model: &str) -> Loaded {
    Loaded {
        file: file.into(),
        cs: CsFile {
            meta: Meta {
                model: model.into(),
                ..Default::default()
            },
            entry: entries,
        },
    }
}

#[test]
fn entries_in_any_cs_file_are_not_proposed_again() {
    let fresh = vec![
        entry("sleep", "years off", 3, Status::Flagged),
        entry("sleep", "ears of", 2, Status::Flagged),
        entry("wake", "hears on", 2, Status::Flagged),
        entry("sleep", "yourself", 2, Status::Flagged),
    ];
    let files = vec![
        // Rejected in a user file for another model: still known.
        loaded(
            "/cfg/cs.d/probe.toml",
            vec![entry("sleep", "Years Off!", 1, Status::Rejected)],
            "qwen3-asr-1.7b",
        ),
        loaded(
            "any/commands.toml",
            vec![entry(" wake ", "hears on", 1, Status::Active)],
            "any",
        ),
        // The review file itself: its flagged entries are merged, not skipped.
        loaded(
            "/cfg/cs.d/mined.toml",
            vec![entry("sleep", "ears of", 1, Status::Flagged)],
            "any",
        ),
    ];
    let kept = unknown(fresh, &files, Some(Path::new("/cfg/cs.d/mined.toml")));
    let heard: Vec<&str> = kept.iter().map(|e| e.heard.as_str()).collect();
    assert_eq!(heard, ["ears of", "yourself"]);
}

#[test]
fn run_skips_what_cs_d_already_has() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join(JOURNAL);
    let t = now() - 60.0;
    let lines = [
        line_at(t, "standby", "years off", None),
        line_at(t + 1.0, "standby", "ears off", Some(("sleep", "ears off"))),
        line_at(t + 2.0, "standby", "hears on", None),
        line_at(t + 3.0, "standby", "ears on", Some(("wake", "ears on"))),
        line_at(t + 30.0, "standby", "years off", None),
        line_at(t + 31.0, "standby", "ears off", Some(("sleep", "ears off"))),
        line_at(t + 32.0, "standby", "hears on", None),
        line_at(t + 33.0, "standby", "ears on", Some(("wake", "ears on"))),
    ];
    let text: String = lines
        .iter()
        .map(|l| serde_json::to_string(l).unwrap() + "\n")
        .collect();
    std::fs::write(&journal, text).unwrap();
    let mut config: crate::config::Config = toml::from_str("").unwrap();
    config.config_dir = dir.path().join("config");
    let cs_d = config.config_dir.join("cs.d");
    std::fs::create_dir_all(&cs_d).unwrap();
    let migrated = CsFile {
        meta: Meta::default(),
        entry: vec![entry("sleep", "years off", 1, Status::Rejected)],
    };
    std::fs::write(cs_d.join("migrated.toml"), migrated.to_toml().unwrap()).unwrap();
    let args = MineArgs {
        journal: Some(journal),
        ..Default::default()
    };
    run(&config, &args).unwrap();
    let out = cs_d.join("mined.toml");
    let file = CsFile::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let heard: Vec<&str> = file.entry.iter().map(|e| e.heard.as_str()).collect();
    assert_eq!(heard, ["hears on"]);
    // Mined again, its own flagged entry is refreshed, not skipped.
    run(&config, &args).unwrap();
    let file = CsFile::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(file.entry.len(), 1);
    assert_eq!(file.entry[0].count, 2);
    assert_eq!(file.entry[0].status, Status::Flagged);
}
