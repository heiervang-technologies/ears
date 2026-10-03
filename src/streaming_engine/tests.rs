use super::ghost::{Partial, StreamLink};
use super::*;
use std::time::Duration;

fn drain(rx: &mut mpsc::UnboundedReceiver<StreamingEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(format!("{:?}", ev));
    }
    out
}

/// Engine wired to a detector with short (3-frame) thresholds and an
/// event receiver, driven by injected probabilities.
fn seq_engine() -> (StreamingEngine, mpsc::UnboundedReceiver<StreamingEvent>) {
    seq_engine_at("http://localhost:8178", PathBuf::new())
}

fn seq_engine_at(
    whisper_url: &str,
    temp_dir: PathBuf,
) -> (StreamingEngine, mpsc::UnboundedReceiver<StreamingEvent>) {
    let mut engine = StreamingEngine::new(
        Arc::new(WhisperClient::new(whisper_url)),
        StreamingConfig::default(),
        VadConfig {
            min_speech_duration_ms: 96,
            max_silence_duration_ms: 96,
            pre_speech_buffer_ms: 64,
            ..VadConfig::default()
        },
        ProgressiveTypingConfig::default(),
        temp_dir,
    )
    .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    engine.set_event_sender(tx);
    (engine, rx)
}

fn feed(engine: &mut StreamingEngine, probs: &[f32]) -> Vec<SpeechSegment> {
    let mut segs = Vec::new();
    for &p in probs {
        let outcome = engine.vad_detector.inject_probability(p);
        if let Some(seg) = engine.handle_vad_outcome(outcome) {
            segs.push(seg);
        }
    }
    segs
}

#[test]
fn test_rejected_candidate_emits_speech_rejected() {
    let (mut engine, mut rx) = seq_engine();
    feed(&mut engine, &[0.9]);
    assert_eq!(drain(&mut rx), vec!["SpeechProbable"]);

    // Silence outlasting the dip tolerance: the candidate is rejected.
    feed(&mut engine, &[0.0, 0.0, 0.0, 0.0]);
    assert_eq!(drain(&mut rx), vec!["SpeechRejected"]);

    // Nothing further while silent.
    feed(&mut engine, &[0.0, 0.0]);
    assert!(drain(&mut rx).is_empty());
}

#[test]
fn test_confirmed_speech_emits_started_and_ended_not_rejected() {
    let (mut engine, mut rx) = seq_engine();
    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    assert_eq!(segs.len(), 1);
    assert_eq!(
        drain(&mut rx),
        vec!["SpeechProbable", "SpeechStarted", "SpeechEnded"]
    );
}

#[test]
fn test_rejected_then_confirmed_sequence() {
    let (mut engine, mut rx) = seq_engine();
    feed(&mut engine, &[0.9, 0.9, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(drain(&mut rx), vec!["SpeechProbable", "SpeechRejected"]);

    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    assert_eq!(segs.len(), 1);
    assert_eq!(
        drain(&mut rx),
        vec!["SpeechProbable", "SpeechStarted", "SpeechEnded"]
    );
}

#[test]
fn test_typing_failure_suppresses_enter_and_suspends_typing() {
    use crate::progressive_typing::ProgressiveTypingError;
    let (mut engine, mut rx) = seq_engine();
    let dir = tempfile::tempdir().unwrap();
    let monitor = crate::health::HealthMonitor::start(dir.path()).unwrap();
    let health = monitor.health();
    engine.set_health(health.clone());
    engine.accumulated_text = "hello world".to_string();

    let delivered = engine.handle_typing_outcome(
        Err(ProgressiveTypingError::TextInputError(
            "child process timed out".to_string(),
        )),
        Instant::now(),
    );

    assert!(
        !delivered,
        "Enter must not follow a failed/timed-out typing"
    );
    assert!(engine.typing_suspended());
    assert!(health.snapshot().typing_paused);
    assert_eq!(
        engine.committed_text(),
        "hello world",
        "transcript history is preserved"
    );
    assert!(engine.progressive_typing.typed_text().is_empty());
    let events = drain(&mut rx);
    assert_eq!(events.len(), 1);
    assert!(events[0].starts_with("Error("), "{}", events[0]);
    assert!(events[0].contains("not retried"));
    assert!(events[0].contains("restart listening"));

    // A later successful-looking outcome does not lift the suspension.
    engine.handle_typing_outcome(Ok(3), Instant::now());
    assert!(engine.typing_suspended());

    // Explicit resume starts clean.
    engine.resume_typing();
    assert!(!engine.typing_suspended());
    assert!(!health.snapshot().typing_paused);
    assert!(engine.committed_text().is_empty());
}

#[test]
fn test_reset_clears_typing_suspension() {
    let (mut engine, _rx) = seq_engine();
    engine.suspend_typing();
    assert!(engine.typing_suspended());
    engine.reset();
    assert!(!engine.typing_suspended());
}

#[test]
fn test_typing_success_keeps_state_and_allows_enter() {
    let (mut engine, mut rx) = seq_engine();
    engine.accumulated_text = "hello".to_string();
    let delivered = engine.handle_typing_outcome(Ok(5), Instant::now());
    assert!(delivered);
    assert_eq!(engine.committed_text(), "hello");
    assert_eq!(engine.stats().chars_typed, 5);
    assert!(drain(&mut rx).is_empty());
}

/// Fake `earsghost` addon: records every line, answers "OK preedit".
fn fake_ghost_addon(dir: &std::path::Path) -> (PathBuf, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, Write};
    let path = dir.join("ghost.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let mut w = stream.try_clone().unwrap();
            for line in std::io::BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let reply: &[u8] = if line == "S" {
                    b"OK preedit TestApp\n"
                } else {
                    b"OK preedit\n"
                };
                let _ = tx.send(line);
                let _ = w.write_all(reply);
            }
        }
    });
    (path, rx)
}

fn ghost_engine(dir: &std::path::Path) -> (StreamingEngine, std::sync::mpsc::Receiver<String>) {
    ghost_engine_at(dir, "http://localhost:8178")
}

/// Ghost engine whose final transcriptions go to `whisper_url`.
fn ghost_engine_at(
    dir: &std::path::Path,
    whisper_url: &str,
) -> (StreamingEngine, std::sync::mpsc::Receiver<String>) {
    let (mut engine, _rx) = seq_engine_at(whisper_url, dir.to_path_buf());
    engine.typing_mode = TypingMode::Wtype;
    let (path, lines) = fake_ghost_addon(dir);
    engine.set_ghost_client(Some(crate::ghost::GhostClient::with_focus_probe(
        path,
        || Some("TestApp".to_string()),
    )));
    assert_eq!(lines.recv().unwrap(), "S", "probe on enable");
    (engine, lines)
}

/// Next command the engine sent, skipping focus status queries.
fn next_cmd(lines: &std::sync::mpsc::Receiver<String>) -> String {
    loop {
        let line = lines.recv().unwrap();
        if line != "S" {
            return line;
        }
    }
}

#[test]
fn test_ghost_commits_are_spaced_and_clear_nothing_extra() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.ghost_commit("hello world");
    engine.ghost_commit("second one");
    assert_eq!(next_cmd(&lines), "C hello world");
    assert_eq!(next_cmd(&lines), "C  second one");
    assert_eq!(engine.stats().chars_typed, 11 + 11);
}

#[test]
fn test_ghost_rejected_candidate_clears_visible_ghost() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    // Simulate a visible partial, then a rejected candidate.
    engine.ghost.as_mut().unwrap().showing = true;
    feed(&mut engine, &[0.9]);
    feed(&mut engine, &[0.0, 0.0, 0.0, 0.0]);
    assert_eq!(next_cmd(&lines), "X");
    assert!(!engine.ghost.as_ref().unwrap().showing);
}

#[test]
fn test_ghost_stale_partials_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    feed(&mut engine, &[0.9, 0.9, 0.9]); // confirmed speech
    let ghost = engine.ghost.as_mut().unwrap();
    let current = ghost.utterance;
    ghost
        .partial_tx
        .send(Partial::plain(current + 7, Ok("old".into())))
        .unwrap();
    ghost
        .partial_tx
        .send(Partial::plain(current, Ok("fresh".into())))
        .unwrap();
    engine.ghost_poll_partials();
    assert_eq!(next_cmd(&lines), "P fresh");
    assert!(
        lines.try_iter().all(|l| l == "S"),
        "stale partial must not be shown"
    );
}

fn spec() -> crate::continuous::ContinuousSpec {
    crate::continuous::ContinuousSpec {
        server_url: "http://127.0.0.1:9".into(),
        api_key: None,
        model: Some("Qwen/Qwen3-ASR-1.7B".into()),
        language: Some("en".into()),
        context: None,
        rollback_words: None,
        min_step_ms: None,
    }
}

#[test]
fn test_ghost_continuous_decoder_follows_its_utterance() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.set_continuous(Some(spec()));
    feed(&mut engine, &[0.9, 0.9, 0.9]); // confirmed speech
    let ghost = engine.ghost.as_mut().unwrap();
    let current = ghost.utterance;
    let mut stale = Partial::plain(current + 1, Ok("old".into()));
    stale.decoder = Some(spec().decoder());
    ghost.partial_tx.send(stale).unwrap();
    engine.ghost_poll_partials();
    assert!(
        engine.ghost.as_ref().unwrap().decoder.is_none(),
        "a stale utterance's decoder is dropped"
    );
    let ghost = engine.ghost.as_mut().unwrap();
    let mut fresh = Partial::plain(current, Ok("fresh".into()));
    fresh.decoder = Some(spec().decoder());
    ghost.partial_tx.send(fresh).unwrap();
    engine.ghost_poll_partials();
    assert_eq!(next_cmd(&lines), "P fresh");
    assert!(engine.ghost.as_ref().unwrap().decoder.is_some());
    // The next utterance starts from a fresh decoder.
    engine.ghost.as_mut().unwrap().next_utterance();
    assert!(engine.ghost.as_ref().unwrap().decoder.is_none());
}

#[test]
fn test_ghost_unsupported_server_falls_back_to_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, _lines) = ghost_engine(dir.path());
    engine.set_continuous(Some(spec()));
    let ghost = engine.ghost.as_mut().unwrap();
    let mut refused = Partial::plain(ghost.utterance, Err("unsupported".into()));
    refused.unsupported = true;
    ghost.partial_tx.send(refused).unwrap();
    engine.ghost_poll_partials();
    assert!(!engine.continuous_http);
}

#[test]
fn test_ghost_muting_clears_visible_ghost() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.ghost.as_mut().unwrap().showing = true;
    engine.set_typing_enabled(false, false, TypingMode::None, false);
    assert_eq!(next_cmd(&lines), "X");
}

#[test]
fn test_ghost_reset_drops_old_partials() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.ghost.as_mut().unwrap().showing = true;
    let old = engine.ghost.as_ref().unwrap().utterance;
    engine.reset();
    assert_eq!(next_cmd(&lines), "X");
    feed(&mut engine, &[0.9, 0.9, 0.9]);
    engine
        .ghost
        .as_ref()
        .unwrap()
        .partial_tx
        .send(Partial::plain(old, Ok("stale".into())))
        .unwrap();
    engine.ghost_poll_partials();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        lines.try_iter().all(|l| l == "S"),
        "pre-reset partial shown"
    );
}

#[test]
fn test_ghost_respects_typing_switch_off() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.typing_mode = TypingMode::None;
    feed(&mut engine, &[0.9, 0.9, 0.9]);
    let current = engine.ghost.as_ref().unwrap().utterance;
    engine
        .ghost
        .as_ref()
        .unwrap()
        .partial_tx
        .send(Partial::plain(current, Ok("secret".into())))
        .unwrap();
    engine.ghost_poll_partials();
    engine.ghost_commit("secret");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(lines.try_recv().is_err(), "nothing may reach the app");
}

fn spec_at(url: &str) -> crate::continuous::ContinuousSpec {
    crate::continuous::ContinuousSpec {
        server_url: url.into(),
        ..spec()
    }
}

/// What `process_audio` does for the ghost between segments, then a
/// moment for the socket tasks to run.
async fn ghost_tick(engine: &mut StreamingEngine) {
    engine.ghost_poll_partials();
    engine.ghost_maybe_start_partial();
    tokio::time::sleep(Duration::from_millis(5)).await;
}

async fn tick_until(engine: &mut StreamingEngine, done: impl Fn(&StreamingEngine) -> bool) {
    for _ in 0..400 {
        if done(engine) {
            return;
        }
        ghost_tick(engine).await;
    }
    panic!("condition never held");
}

fn stream_up(engine: &StreamingEngine) -> bool {
    matches!(engine.ghost.as_ref().unwrap().stream, StreamLink::Up(_))
}

/// Tick until the addon gets a command other than a status query.
async fn next_cmd_ticking(
    engine: &mut StreamingEngine,
    lines: &std::sync::mpsc::Receiver<String>,
) -> String {
    for _ in 0..400 {
        ghost_tick(engine).await;
        if let Some(line) = lines.try_iter().find(|l| l != "S") {
            return line;
        }
    }
    panic!("the ghost addon never got a command");
}

fn stream_partial(id: u64, text: &str, stable_chars: usize) -> serde_json::Value {
    serde_json::json!({"type": "partial", "utterance": id, "seq": 1, "text": text,
            "stable_chars": stable_chars, "language": "English", "audio_ms": 1000,
            "decode_ms": 50})
}

fn transcription(text: &str) -> wiremock::ResponseTemplate {
    wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": text}))
}

fn chat_reply(content: &str) -> wiremock::ResponseTemplate {
    wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "choices": [{"message": {"content": content}, "finish_reason": "stop"}]
    }))
}

#[tokio::test]
async fn test_ghost_stream_utterance_follows_the_segment() {
    use crate::stream_client::fake::{FakeServer, Mode};
    use wiremock::{matchers::path, Mock, MockServer};
    let whisper = MockServer::start().await;
    Mock::given(path("/v1/audio/transcriptions"))
        .respond_with(transcription("hello stream."))
        .mount(&whisper)
        .await;
    let server = FakeServer::start(Mode::Normal).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine_at(dir.path(), &whisper.uri());
    engine.set_continuous(Some(spec_at(&server.url)));
    tick_until(&mut engine, stream_up).await;

    feed(&mut engine, &[0.9, 0.9, 0.9]);
    ghost_tick(&mut engine).await;
    let start = server.wait_for(|l| l.starts_with("start ")).await;
    assert!(start.contains(r#""language":"en""#), "{start}");
    let id = server.starts()[0];
    server.say(stream_partial(id, "hello stream", 5));
    assert_eq!(
        next_cmd_ticking(&mut engine, &lines).await,
        "F 5 hello stream"
    );
    assert_eq!(
        engine
            .ghost
            .as_ref()
            .unwrap()
            .stream_state
            .as_ref()
            .unwrap()
            .stable,
        "hello"
    );

    let segs = feed(&mut engine, &[0.9, 0.0, 0.0, 0.0]);
    assert_eq!(segs.len(), 1);
    let total = segs[0].samples.len();
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    server.wait_for(|l| l == format!("end {id}")).await;
    // Every sample went out exactly once, before `end`.
    assert_eq!(server.audio_bytes(), 2 * total);
    assert_eq!(
        next_cmd_ticking(&mut engine, &lines).await,
        "C hello stream."
    );
    assert_eq!(server.starts(), vec![id]);
}

#[tokio::test]
async fn test_ghost_stream_drops_stale_partials_and_cancels_on_reset() {
    use crate::stream_client::fake::{FakeServer, Mode};
    let server = FakeServer::start(Mode::Normal).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.set_continuous(Some(spec_at(&server.url)));
    tick_until(&mut engine, stream_up).await;

    feed(&mut engine, &[0.9, 0.9, 0.9]);
    ghost_tick(&mut engine).await;
    server.wait_for(|l| l.starts_with("start ")).await;
    let old = server.starts()[0];
    engine.reset();
    server.wait_for(|l| l == format!("cancel {old}")).await;

    feed(&mut engine, &[0.9, 0.9, 0.9]);
    ghost_tick(&mut engine).await;
    server
        .wait_for(|l| l.starts_with("start ") && !l.contains(&format!(":{old},")))
        .await;
    let fresh = server.starts()[1];
    assert!(fresh > old, "stream utterance ids only increase");
    server.say(stream_partial(old, "old words", 0));
    server.say(stream_partial(fresh, "fresh words", 0));
    assert_eq!(next_cmd_ticking(&mut engine, &lines).await, "P fresh words");

    // Muting cancels the stream utterance too.
    engine.set_typing_enabled(false, false, TypingMode::None, false);
    server.wait_for(|l| l == format!("cancel {fresh}")).await;
    ghost_tick(&mut engine).await;
    assert_eq!(server.starts().len(), 2, "no stream while muted");
}

#[tokio::test]
async fn test_ghost_without_stream_endpoint_decodes_per_tick() {
    use wiremock::{matchers::path, Mock, MockServer};
    // wiremock answers the upgrade with 404: the plugin is not installed.
    let server = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(chat_reply("over http"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.set_continuous(Some(spec_at(&server.uri())));
    tick_until(&mut engine, |e| {
        matches!(e.ghost.as_ref().unwrap().stream, StreamLink::Unsupported)
    })
    .await;
    feed(&mut engine, &[0.9; 24]);
    assert_eq!(next_cmd_ticking(&mut engine, &lines).await, "P over http");
}

#[tokio::test]
async fn test_ghost_lost_stream_falls_back_without_duplicate_commit() {
    use crate::stream_client::fake::{FakeServer, Mode};
    use wiremock::{matchers::path, Mock, MockServer};
    let http = MockServer::start().await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(chat_reply(" then http"))
        .mount(&http)
        .await;
    Mock::given(path("/v1/audio/transcriptions"))
        .respond_with(transcription("streamed words then http."))
        .mount(&http)
        .await;
    // One URL for both: the stream here, plain HTTP passed to wiremock.
    let server = FakeServer::start_with_http(Mode::Normal, Some(*http.address())).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine_at(dir.path(), &server.url);
    engine.set_continuous(Some(spec_at(&server.url)));
    tick_until(&mut engine, stream_up).await;

    feed(&mut engine, &[0.9; 24]);
    ghost_tick(&mut engine).await;
    server.wait_for(|l| l.starts_with("start ")).await;
    let id = server.starts()[0];
    server.say(stream_partial(id, "streamed words so far", 14));
    assert_eq!(
        next_cmd_ticking(&mut engine, &lines).await,
        "F 14 streamed words so far"
    );

    server.drop_connections();
    // The per-tick decoder carries on from the settled text.
    assert_eq!(
        next_cmd_ticking(&mut engine, &lines).await,
        "F 14 streamed words then http"
    );
    let requests = http.received_requests().await.unwrap();
    let chat: serde_json::Value = serde_json::from_slice(
        &requests
            .iter()
            .find(|r| r.url.path() == "/v1/chat/completions")
            .unwrap()
            .body,
    )
    .unwrap();
    let messages = chat["messages"].as_array().unwrap();
    assert_eq!(
        messages.last().unwrap()["content"],
        "language English<asr_text>streamed words"
    );

    let segs = feed(&mut engine, &[0.0, 0.0, 0.0]);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    let commits: Vec<String> = lines.try_iter().filter(|l| l.starts_with("C ")).collect();
    assert_eq!(commits, vec!["C streamed words then http."]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let finals = http
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/v1/audio/transcriptions")
        .count();
    assert_eq!(finals, 1);
    // The utterance was never replayed into a new stream.
    assert_eq!(server.starts(), vec![id]);
}

#[test]
fn test_guided_segments_are_discrete_without_progressive_typing() {
    let (mut engine, _rx) = seq_engine();
    assert!(!engine.config.progressive_typing);
    engine.set_guided_grammar(Some("root ::= command".to_string()));

    assert_eq!(engine.commit_transcript("git status"), "git status");
    assert_eq!(engine.committed_text(), "git status");

    assert_eq!(engine.commit_transcript("cargo test"), "cargo test");
    assert_eq!(engine.committed_text(), "cargo test");
}

#[test]
fn test_dictation_segments_still_accumulate() {
    let (mut engine, _rx) = seq_engine();
    engine.commit_transcript("hello");
    engine.commit_transcript("world");
    assert_eq!(engine.committed_text(), "hello world");
}

#[test]
fn test_switching_guided_mode_clears_typing_state() {
    let (mut engine, _rx) = seq_engine();
    engine.accumulated_text = "previous dictation".to_string();

    engine.set_guided_grammar(Some("root ::= command".to_string()));
    assert!(engine.committed_text().is_empty());

    // Replacing one grammar with another keeps command state.
    engine.commit_transcript("pwd");
    engine.set_guided_grammar(Some("root ::= other".to_string()));
    assert_eq!(engine.committed_text(), "pwd");

    engine.set_guided_grammar(None);
    assert!(engine.committed_text().is_empty());
}

#[test]
fn test_streaming_stats_default() {
    let stats = StreamingStats::default();
    assert_eq!(stats.segments_processed, 0);
    assert_eq!(stats.avg_latency_ms, 0);
    assert_eq!(stats.chars_typed, 0);
}

#[test]
fn kept_segment_audio_is_a_16k_mono_wav_of_the_samples() {
    let dir = tempfile::tempdir().unwrap();
    let samples = vec![0.25f32; 1600];
    let path = keep_segment_audio(dir.path(), &samples).expect("clip kept");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 1); // mono
    assert_eq!(
        u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        16000
    );
    assert_eq!(bytes.len(), 44 + samples.len() * 2);
    assert!(std::path::Path::new(&path).starts_with(dir.path()));
}

#[test]
fn kept_segment_audio_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let unrelated = dir.path().join("notes.wav");
    std::fs::write(&unrelated, b"x").unwrap();
    let mut paths = Vec::new();
    for _ in 0..KEEP_AUDIO_MAX + 3 {
        paths.push(keep_segment_audio(dir.path(), &[0.0; 160]).unwrap());
        // Distinct mtimes so "oldest" is well defined.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let kept = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("utterance_")
        })
        .count();
    assert_eq!(kept, KEEP_AUDIO_MAX);
    assert!(!std::path::Path::new(&paths[0]).exists(), "oldest pruned");
    assert!(std::path::Path::new(paths.last().unwrap()).exists());
    assert!(unrelated.exists(), "only our clips are pruned");
}

#[test]
fn segment_completed_serializes_audio_path_only_when_kept() {
    let plain = serde_json::to_value(StreamingEvent::SegmentCompleted {
        text: "hi".into(),
        duration_ms: 5,
        audio_path: None,
    })
    .unwrap();
    assert_eq!(
        plain,
        serde_json::json!({"SegmentCompleted": {"text": "hi", "duration_ms": 5}})
    );
    let kept = serde_json::to_value(StreamingEvent::SegmentCompleted {
        text: "hi".into(),
        duration_ms: 5,
        audio_path: Some("/x/utterance_1.wav".into()),
    })
    .unwrap();
    assert_eq!(kept["SegmentCompleted"]["audio_path"], "/x/utterance_1.wav");
}

#[tokio::test]
async fn test_keep_audio_reports_the_utterance_wav_on_segment_completed() {
    use wiremock::{matchers::path, Mock, MockServer};
    let whisper = MockServer::start().await;
    Mock::given(path("/v1/audio/transcriptions"))
        .respond_with(transcription("hello jasonelle"))
        .mount(&whisper)
        .await;
    let work = tempfile::tempdir().unwrap();
    let keep = tempfile::tempdir().unwrap();
    let (mut engine, mut rx) = seq_engine_at(&whisper.uri(), work.path().to_path_buf());
    engine.set_keep_audio_dir(Some(keep.path().to_path_buf()));

    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    assert_eq!(segs.len(), 1);
    let samples = segs[0].samples.len();
    assert!(samples > 0);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();

    let mut audio = None;
    while let Ok(ev) = rx.try_recv() {
        if let StreamingEvent::SegmentCompleted {
            text, audio_path, ..
        } = ev
        {
            assert_eq!(text, "hello jasonelle");
            audio = audio_path;
        }
    }
    let audio = audio.expect("SegmentCompleted carries audio_path");
    assert!(std::path::Path::new(&audio).starts_with(keep.path()));
    // The kept clip is exactly the segment ASR heard: 44-byte header + s16.
    assert_eq!(
        std::fs::metadata(&audio).unwrap().len() as usize,
        44 + samples * 2
    );
    // The ASR temp file is still cleaned up.
    assert_eq!(std::fs::read_dir(work.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn test_without_keep_audio_no_audio_path_and_no_clip() {
    use wiremock::{matchers::path, Mock, MockServer};
    let whisper = MockServer::start().await;
    Mock::given(path("/v1/audio/transcriptions"))
        .respond_with(transcription("hello"))
        .mount(&whisper)
        .await;
    let work = tempfile::tempdir().unwrap();
    let (mut engine, mut rx) = seq_engine_at(&whisper.uri(), work.path().to_path_buf());
    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    let mut seen = false;
    while let Ok(ev) = rx.try_recv() {
        if let StreamingEvent::SegmentCompleted { audio_path, .. } = ev {
            assert!(audio_path.is_none());
            seen = true;
        }
    }
    assert!(seen);
    assert_eq!(std::fs::read_dir(work.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn test_unlikely_segment_is_dropped_before_transcription() {
    use wiremock::{matchers::path, Mock, MockServer};
    let whisper = MockServer::start().await;
    Mock::given(path("/v1/audio/transcriptions"))
        .respond_with(transcription("Thank you."))
        .expect(0)
        .mount(&whisper)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, mut rx) = seq_engine_at(&whisper.uri(), dir.path().to_path_buf());
    engine.min_mean_probability = 0.6;

    // Barely over the start threshold all the way: passes the gate, but its
    // mean (0.55) is below the floor.
    let segs = feed(&mut engine, &[0.55, 0.55, 0.55, 0.0, 0.0, 0.0]);
    assert_eq!(segs.len(), 1);
    assert!((segs[0].mean_probability - 0.55).abs() < 1e-6);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    assert!(engine.accumulated_text.is_empty());
    assert!(
        drain(&mut rx).contains(&"SegmentDiscarded { reason: \"not speech-like enough\" }".into()),
        "a dropped segment is reported, not silent"
    );
}

#[tokio::test]
async fn test_segment_with_no_words_is_reported_as_discarded() {
    let (mut engine, mut rx, _server, _dir) = command_engine("").await;
    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    drain(&mut rx);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    let events = drain(&mut rx);
    assert!(
        events.contains(&"SegmentDiscarded { reason: \"no words heard\" }".into()),
        "{events:?}"
    );
}

async fn command_engine(
    heard: &str,
) -> (
    StreamingEngine,
    mpsc::UnboundedReceiver<StreamingEvent>,
    wiremock::MockServer,
    tempfile::TempDir,
) {
    use wiremock::{matchers::path, Mock, MockServer};
    let whisper = MockServer::start().await;
    Mock::given(path("/v1/audio/transcriptions"))
        .respond_with(transcription(heard))
        .mount(&whisper)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, rx) = seq_engine_at(&whisper.uri(), dir.path().to_path_buf());
    // Never drive the real keyboard from a test.
    engine.typing_mode = TypingMode::None;
    engine.set_commands(crate::commands::VoiceCommands {
        enabled: true,
        enter: vec!["over".into()],
        ..Default::default()
    });
    (engine, rx, whisper, dir)
}

#[tokio::test]
async fn test_command_utterance_is_not_dictation() {
    let (mut engine, mut rx, _server, _dir) = command_engine("Over.").await;
    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    drain(&mut rx);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    assert!(engine.accumulated_text.is_empty(), "a command is not typed");
    let events = drain(&mut rx);
    assert!(
        !events.iter().any(|e| e.starts_with("SegmentCompleted")),
        "a command is never published as dictation: {events:?}"
    );
}

#[tokio::test]
async fn test_command_word_inside_a_sentence_is_dictation() {
    let (mut engine, mut rx, _server, _dir) = command_engine("It's over now.").await;
    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    drain(&mut rx);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    assert_eq!(engine.accumulated_text.trim(), "It's over now.");
    assert!(drain(&mut rx)
        .iter()
        .any(|e| e.starts_with("SegmentCompleted")));
}

#[tokio::test]
async fn test_literal_types_the_command_word() {
    let (mut engine, mut rx, _server, _dir) = command_engine("Literal over.").await;
    let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
    drain(&mut rx);
    engine
        .process_segment(segs.into_iter().next().unwrap())
        .await
        .unwrap();
    assert_eq!(engine.accumulated_text.trim(), "over.");
}

fn over_commands(accept_ms: u64) -> crate::commands::VoiceCommands {
    crate::commands::VoiceCommands {
        enabled: true,
        enter: vec!["over".into()],
        accept_ms,
        ..Default::default()
    }
}

#[test]
fn test_ghost_command_partial_is_drawn_in_accept_colour() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.set_commands(over_commands(200));
    feed(&mut engine, &[0.9, 0.9, 0.9]); // confirmed speech
    let ghost = engine.ghost.as_mut().unwrap();
    let current = ghost.utterance;
    ghost
        .partial_tx
        .send(Partial::plain(current, Ok("Over.".into())))
        .unwrap();
    ghost
        .partial_tx
        .send(Partial::plain(current, Ok("Over and out".into())))
        .unwrap();
    engine.ghost_poll_partials();
    assert_eq!(next_cmd(&lines), "F 5 Over.", "whole command is frozen");
    assert_eq!(next_cmd(&lines), "P Over and out", "dictation is not");
}

#[tokio::test]
async fn test_ghost_accept_flashes_then_clears_without_committing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, lines) = ghost_engine(dir.path());
    engine.set_commands(over_commands(1));
    engine.ghost_accept("Over.").await;
    assert_eq!(next_cmd(&lines), "F 5 Over.");
    assert_eq!(next_cmd(&lines), "X");
    // With no hold, the ghost just goes.
    engine.set_commands(over_commands(0));
    engine.ghost.as_mut().unwrap().showing = true;
    engine.ghost_accept("Over.").await;
    assert_eq!(next_cmd(&lines), "X");
    assert!(lines.try_iter().all(|l| l == "S"), "nothing committed");
}
