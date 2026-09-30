//! Push-to-talk ghost lifecycle, live decoding, observation, and final handoff.

use anyhow::{Context, Result};
use ears::{Config, KeyboardLayout, WhisperClient};
use std::time::Duration;

/// Read-only visualization independent of the target app's preedit styling.
pub(super) async fn watch_ghost(json: bool) -> Result<()> {
    let mut client = ears::ghost::GhostClient::new(ears::ghost::default_socket_path());
    let mut previous = None;
    let mut interval = tokio::time::interval(Duration::from_millis(150));
    if !json {
        eprintln!("Live decoding: FROZEN | revisable. Full final correction may replace the prefix. Ctrl-C to stop.");
    }
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                let state = client.snapshot().context("ghost-watch requires the updated fcitx5 earsghost addon")?;
                if previous.as_ref() == Some(&state) { continue; }
                if json {
                    println!("{}", serde_json::to_string(&state)?);
                } else if state.text.is_empty() {
                    println!("[no ghost]");
                } else {
                    // Debug strings escape terminal control characters in ASR output.
                    println!("FROZEN {:?} | revisable {:?}", &state.text[..state.frozen_bytes], &state.text[state.frozen_bytes..]);
                }
                previous = Some(state);
            }
        }
    }
    Ok(())
}

/// File holding the PID of a running push-to-talk ghost preview.
fn ghost_preview_pid_file(config: &Config) -> std::path::PathBuf {
    config.state_dir.join("ghost-preview.pid")
}

/// File holding the text the ghost preview is currently showing.
fn ghost_preview_text_file(config: &Config) -> std::path::PathBuf {
    config.state_dir.join("ghost-preview.txt")
}

/// Settled continuous-decoding state the preview leaves for the final commit.
pub(super) fn ghost_continuous_state_file(config: &Config) -> std::path::PathBuf {
    config.state_dir.join("ghost-continuous.json")
}

/// Continuous ticks cost about the same at any length; the bound is the
/// server's context (about 17 tokens per second against 2048).
const MAX_CONTINUOUS_BYTES: usize = 16_000 * 2 * 90;

/// Identifies one recorder process in the continuous state file.
pub(super) fn recording_owner(recorder_pid: i32, recorder_start: u64) -> String {
    format!("{} {}", recorder_pid, recorder_start)
}

/// Replace the state file in one step: a preview killed mid-write must not
/// leave half a file behind.
fn write_continuous_state(path: &std::path::Path, state: &ears::continuous::DecoderState) {
    let Ok(json) = serde_json::to_string(state) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Start time of a process in clock ticks since boot (field 22 of
/// `/proc/PID/stat`). Together with the PID this identifies one process:
/// a recycled PID gets a different start time.
pub(super) fn proc_start_time(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // The command name may contain spaces or parentheses; fields resume
    // after the last ')'. starttime is the 20th field from there.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Whether `pid` is still the process that started at `start`.
fn same_process(pid: i32, start: u64) -> bool {
    pid > 0 && proc_start_time(pid) == Some(start)
}

/// Start the ghost preview loop for the recording that just started.
pub(super) fn spawn_ghost_preview(config: &Config, recorder_pid: u32) -> Result<()> {
    let recorder_pid = recorder_pid as i32;
    let recorder_start =
        proc_start_time(recorder_pid).context("recorder exited before the preview started")?;
    // At most one preview: a leftover one would fight over the ghost.
    let _ = stop_ghost_preview(config);

    let exe = std::env::current_exe().context("cannot locate ears executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("ghost-preview")
        .arg("--recorder-pid")
        .arg(recorder_pid.to_string())
        .arg("--recorder-start")
        .arg(recorder_start.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(ref profile) = config.active_profile {
        cmd.env("EARS_PROFILE", profile);
    }
    // New session: the preview must outlive this short-lived toggle process.
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    let mut child = cmd.spawn().context("cannot spawn ghost preview")?;
    let pid = child.id() as i32;
    let record = proc_start_time(pid)
        .map(|start| format!("{} {}", pid, start))
        .context("ghost preview exited immediately")
        .and_then(|line| {
            std::fs::write(ghost_preview_pid_file(config), line)
                .context("cannot record ghost preview PID")
        });
    if let Err(e) = record {
        // Uncontrollable without its PID file: do not leave it running.
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let _ = std::fs::remove_file(ghost_preview_text_file(config));
    // Reap it from a detached thread if we are still around when it exits.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Stop a running ghost preview. Returns `Some(last shown text)` when this
/// recording was a ghost session, `None` otherwise.
///
/// Only signals the process recorded in the PID file if it is still that
/// same process (PID plus start time); a stale file never kills a stranger.
pub(super) fn stop_ghost_preview(config: &Config) -> Option<String> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let pid_file = ghost_preview_pid_file(config);
    let record = std::fs::read_to_string(&pid_file).ok()?;
    let _ = std::fs::remove_file(&pid_file);
    let mut fields = record.split_whitespace().map(|f| f.parse::<u64>().ok());
    if let (Some(Some(pid)), Some(Some(start))) = (fields.next(), fields.next()) {
        let pid = pid as i32;
        if same_process(pid, start) {
            let target = Pid::from_raw(pid);
            let _ = kill(target, Signal::SIGTERM);
            let deadline = std::time::Instant::now() + Duration::from_millis(800);
            while same_process(pid, start) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if same_process(pid, start) {
                let _ = kill(target, Signal::SIGKILL);
            }
        }
    }
    let text_file = ghost_preview_text_file(config);
    let last = std::fs::read_to_string(&text_file).unwrap_or_default();
    let _ = std::fs::remove_file(&text_file);
    let _ = std::fs::remove_file(config.state_dir.join("ghost_toggle_partial.wav"));
    Some(last)
}

/// Commit the final push-to-talk text through the ghost addon.
pub(super) fn deliver_via_ghost(last_ghost: &str, text: &str) -> ears::ghost::Delivery {
    use ears::ghost::{default_socket_path, Delivery, GhostClient};
    let mut client = GhostClient::new(default_socket_path());
    if !last_ghost.is_empty() {
        // Bridge the gap left by the stopped preview's disconnect.
        let _ = client.preedit(last_ghost);
    }
    let delivery = client.commit(text);
    match delivery {
        Delivery::Delivered => {}
        Delivery::NotDelivered => {
            tracing::info!("Ghost addon did not deliver the text; typing instead")
        }
        Delivery::Unknown => {
            tracing::warn!("Ghost commit outcome unknown; not typing it again")
        }
    }
    delivery
}

/// Ghost text of a push-to-talk preview: filtered, deduplicated, and
/// mirrored to the file the stopping toggle reads.
struct PreviewGhost<'a> {
    config: &'a Config,
    language: Option<String>,
    client: ears::ghost::GhostClient,
    text_file: std::path::PathBuf,
    last_shown: String,
    last_frozen: usize,
}

impl PreviewGhost<'_> {
    fn show(&mut self, text: String, frozen_bytes: usize) {
        let (text, frozen_bytes) = if self.config.bash_mode {
            (text, 0)
        } else {
            ears::freeze::filtered(
                &text,
                frozen_bytes,
                &self.config.text_filters,
                self.language.as_deref(),
            )
        };
        if text == self.last_shown && frozen_bytes == self.last_frozen {
            return;
        }
        match self.client.preedit_frozen(&text, frozen_bytes) {
            Ok(_) => {
                self.last_frozen = frozen_bytes;
                self.last_shown = text;
                let _ = std::fs::write(&self.text_file, &self.last_shown);
            }
            Err(e) => tracing::debug!("Ghost preview: {}", e),
        }
    }
}

/// How a stream preview ended.
enum StreamPreview {
    /// The recording ended with the stream still working.
    Done,
    /// No stream, or it was lost: continue per tick from this settled state.
    Fallback(ears::continuous::DecoderState),
}

/// Ghost preview over the ears stream: tail the growing recording every
/// 50 ms, send only the new samples, and show each partial as it arrives.
/// After each partial the settled state goes to the state file, as the
/// per-tick decoder leaves it, so the stop path works the same either way.
async fn stream_ghost_preview(
    config: &Config,
    server_url: &str,
    language: Option<&str>,
    owner: &str,
    recorder_alive: impl Fn() -> bool,
    ghost: &mut PreviewGhost<'_>,
) -> StreamPreview {
    use ears::continuous::{join_segments, samples, DecoderState, Rollover, PAUSE_WINDOW};
    use ears::stream_client::{StartParams, StreamEvent, StreamSession};

    const POLL: Duration = Duration::from_millis(50);

    let state_file = ghost_continuous_state_file(config);
    let mut state = DecoderState::default();
    let mut session = match StreamSession::connect(server_url, config.api_key.as_deref()).await {
        Ok(session) => session,
        Err(e) => {
            tracing::info!("Stream unavailable, decoding per tick: {}", e);
            return StreamPreview::Fallback(state);
        }
    };
    let params = StartParams {
        language: language.map(str::to_string),
        context: config.prompt.clone().filter(|c| !c.trim().is_empty()),
        ..StartParams::default()
    };
    // One utterance per segment; a long recording rolls over to the next
    // before it outgrows the server's context.
    let rollover = session
        .max_samples()
        .map_or(Rollover::DEFAULT, |max| Rollover {
            soft: Rollover::DEFAULT.soft.min(max * 2 / 3),
            hard: Rollover::DEFAULT.hard.min(max * 9 / 10),
        });
    let mut utterance = 1u64;
    session.start(utterance, &params);
    let mut tail = ears::ghost::WavTail::new(config.state_dir.join("recording.wav"));
    // Samples in the current segment, and those read while the previous
    // segment's final was pending.
    let mut segment = 0usize;
    let mut pending: Vec<u8> = Vec::new();
    let mut ending = false;
    let mut recent: Vec<i16> = Vec::with_capacity(PAUSE_WINDOW * 2);
    let mut poll = tokio::time::interval(POLL);
    while recorder_alive() {
        tokio::select! {
            _ = poll.tick() => {
                let Ok(bytes) = tail.read_new() else {
                    continue;
                };
                if ending {
                    pending.extend_from_slice(&bytes);
                    continue;
                }
                // A refused push means the connection is gone; its Closed
                // event ends the loop.
                if !bytes.is_empty() && session.push_bytes(utterance, &bytes) {
                    segment += bytes.len() / 2;
                    recent.extend(samples(&bytes));
                    let excess = recent.len().saturating_sub(PAUSE_WINDOW);
                    recent.drain(..excess);
                }
                if rollover.due(segment, &recent) && session.end(utterance) {
                    tracing::info!("Stream segment rolls over after {} samples", segment);
                    ending = true;
                }
            }
            event = session.recv() => match event {
                Some(StreamEvent::Partial(partial)) if partial.utterance == utterance => {
                    if !recorder_alive() {
                        break; // a later recording owns the state file now
                    }
                    let prefix = std::mem::take(&mut state.prefix);
                    let offset = state.offset;
                    state = partial.snapshot();
                    state.prefix = prefix;
                    state.offset = offset;
                    state.owner = Some(owner.to_string());
                    write_continuous_state(&state_file, &state);
                    let text = join_segments(&state.prefix, partial.text.trim());
                    let frozen = join_segments(&state.prefix, &state.stable);
                    ghost.show(text.clone(), if text.starts_with(&frozen) { frozen.len() } else { 0 });
                }
                Some(StreamEvent::Final { utterance: done, text }) if done == utterance && ending => {
                    // The finished segment's text is now fixed; the next one
                    // starts where it ended.
                    state = DecoderState {
                        header: state.header.take(),
                        prefix: join_segments(&state.prefix, &text),
                        offset: state.offset + segment,
                        owner: Some(owner.to_string()),
                        ..DecoderState::default()
                    };
                    write_continuous_state(&state_file, &state);
                    ghost.show(state.prefix.clone(), state.prefix.len());
                    utterance += 1;
                    segment = 0;
                    ending = false;
                    recent.clear();
                    session.start(utterance, &params);
                    let backlog = std::mem::take(&mut pending);
                    if !backlog.is_empty() && session.push_bytes(utterance, &backlog) {
                        segment += backlog.len() / 2;
                    }
                }
                Some(StreamEvent::Error { code, message, .. }) if code == "unsupported" => {
                    tracing::warn!("Stream unsupported, decoding per tick: {}", message);
                    return StreamPreview::Fallback(state);
                }
                Some(StreamEvent::Error { utterance: Some(u), code, message }) if u == utterance && ending => {
                    // The segment's final failed: its text is lost to the
                    // preview, so hand over to per-tick decoding, which
                    // decodes the segment again from its settled text.
                    tracing::warn!("Stream segment final failed ({}): {}", code, message);
                    return StreamPreview::Fallback(state);
                }
                Some(StreamEvent::Error { code, message, .. }) => {
                    tracing::debug!("Stream error {}: {}", code, message);
                }
                Some(StreamEvent::Closed(reason)) => {
                    tracing::warn!("Stream lost ({}), decoding per tick", reason);
                    return StreamPreview::Fallback(state);
                }
                None => return StreamPreview::Fallback(state),
                Some(_) => {}
            }
        }
    }
    StreamPreview::Done
}

/// Ghost preview for push-to-talk: while the recording grows, transcribe it
/// and show the result as inline ghost text. With continuous decoding the
/// ears stream is tried first, then per-tick HTTP continuous decoding, then
/// repeated transcriptions every few hundred milliseconds.
/// Exits when the recording ends (or on SIGTERM from the stopping toggle);
/// closing the addon connection clears the ghost.
pub(super) async fn run_ghost_preview(
    config: &Config,
    recorder_pid: i32,
    recorder_start: u64,
) -> Result<()> {
    use ears::continuous::{
        join_segments, samples, ContinuousDecoder, ContinuousError, DecoderState, Rollover,
    };
    use ears::ghost::{default_socket_path, growing_wav_payload, write_pcm16_wav, GhostClient};

    const INTERVAL: Duration = Duration::from_millis(300);
    const MIN_BYTES: usize = 16_000 * 2 * 4 / 10; // 0.4 s
    const MAX_BYTES: usize = 16_000 * 2 * 60; // stop previewing past 60 s
    const DEADLINE: Duration = Duration::from_secs(4);

    let audio_file = config.state_dir.join("recording.wav");
    let partial = config.state_dir.join("ghost_toggle_partial.wav");
    let mut partial_cleanup = None;

    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());
    let client = WhisperClient::new(server_url.to_string())
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(model.clone())
        .with_prompt(config.prompt.clone());
    let grammar = config.active_grammar();
    let mut ghost = PreviewGhost {
        config,
        language: language.clone(),
        client: GhostClient::new(default_socket_path()),
        text_file: ghost_preview_text_file(config),
        last_shown: String::new(),
        last_frozen: 0,
    };
    // Grammar-constrained (bash) decoding has its own request shape.
    let mut continuous = (config.live_decoding == ears::config::LiveDecoding::Continuous
        && grammar.is_none())
    .then(|| {
        ContinuousDecoder::new(server_url.as_str(), config.api_key.clone(), model)
            .with_language(language.as_deref())
            .with_context(config.prompt.clone())
    });
    let state_file = ghost_continuous_state_file(config);
    let owner = recording_owner(recorder_pid, recorder_start);

    // Bound to the one recorder it was started for: a later recording (even
    // one reusing the PID) is not ours to preview.
    let recorder_alive = || same_process(recorder_pid, recorder_start);

    // Finished segments of a long recording (see `Rollover`).
    let mut prefix = String::new();
    let mut offset = 0usize;
    let mut finished = false;
    if let Some(decoder) = continuous.take() {
        match stream_ghost_preview(
            config,
            server_url.as_str(),
            language.as_deref(),
            &owner,
            recorder_alive,
            &mut ghost,
        )
        .await
        {
            StreamPreview::Done => finished = true,
            // Carry on from what the stream settled; nothing is replayed.
            StreamPreview::Fallback(state) => {
                prefix = state.prefix.clone();
                offset = state.offset;
                continuous = Some(decoder.resume(state));
            }
        }
    }

    let mut last_len = 0usize;
    while !finished && recorder_alive() {
        tokio::time::sleep(INTERVAL).await;
        let Ok(bytes) = std::fs::read(&audio_file) else {
            continue;
        };
        let Some(pcm) = growing_wav_payload(&bytes) else {
            continue;
        };
        if pcm.len() < MIN_BYTES || pcm.len() == last_len {
            continue;
        }
        if continuous.is_none() && pcm.len() > MAX_BYTES {
            continue;
        }
        last_len = pcm.len();
        let text = if let Some(decoder) = continuous.as_mut() {
            let all = samples(pcm);
            let segment = &all[offset.min(all.len())..];
            // Finish the segment at a pause before it outgrows the context.
            let roll = Rollover::DEFAULT.due(segment.len(), segment);
            let outcome = decoder.step(segment, roll, DEADLINE).await;
            if !recorder_alive() {
                break; // a later recording owns the state file now
            }
            match outcome {
                Ok(text) if roll => {
                    tracing::info!("Segment rolls over after {} samples", segment.len());
                    prefix = join_segments(&prefix, &text);
                    offset += segment.len();
                    let header = decoder.snapshot().header;
                    *decoder = ContinuousDecoder::new(
                        server_url.as_str(),
                        config.api_key.clone(),
                        decoder.model().map(str::to_string),
                    )
                    .with_language(language.as_deref())
                    .with_context(config.prompt.clone())
                    .resume(DecoderState {
                        header,
                        ..Default::default()
                    });
                    let state = DecoderState {
                        prefix: prefix.clone(),
                        offset,
                        owner: Some(owner.clone()),
                        ..decoder.snapshot()
                    };
                    write_continuous_state(&state_file, &state);
                    prefix.clone()
                }
                Ok(text) => {
                    let mut state = decoder.snapshot();
                    state.owner = Some(owner.clone());
                    state.prefix = prefix.clone();
                    state.offset = offset;
                    write_continuous_state(&state_file, &state);
                    join_segments(&prefix, &text)
                }
                Err(ContinuousError::Unsupported(e)) => {
                    let state = ears::continuous::DecoderState {
                        owner: Some(owner.clone()),
                        unsupported: true,
                        ..Default::default()
                    };
                    write_continuous_state(&state_file, &state);
                    tracing::warn!("Continuous decoding unavailable, repeating instead: {}", e);
                    continuous = None;
                    last_len = 0;
                    continue;
                }
                Err(e) => {
                    tracing::debug!("Ghost preview transcription: {}", e);
                    continue;
                }
            }
        } else {
            if let Err(error) = write_pcm16_wav(&partial, pcm, 16_000) {
                tracing::debug!(%error, "Failed to write preview WAV");
                // A failed write can still have created a partial file.
                if partial_cleanup.is_none() {
                    drop(crate::owned_file::CleanupFile::new(&partial)?);
                }
                continue;
            }
            if partial_cleanup.is_none() {
                partial_cleanup = Some(crate::owned_file::CleanupFile::new(&partial)?);
            }
            match client
                .transcribe_preview(&partial, grammar.as_deref(), DEADLINE)
                .await
            {
                Ok(text) => text,
                Err(e) => {
                    tracing::debug!("Ghost preview transcription: {}", e);
                    continue;
                }
            }
        };
        if !recorder_alive() {
            continue;
        }
        let frozen = continuous
            .as_ref()
            .map(|d| join_segments(&prefix, &d.snapshot().stable))
            .unwrap_or_default();
        let frozen_bytes = if text.starts_with(&frozen) {
            frozen.len()
        } else {
            0
        };
        ghost.show(text, frozen_bytes);
    }
    drop(partial_cleanup);
    // Natural exit: drop our PID record so nothing signals a recycled PID.
    let pid_file = ghost_preview_pid_file(config);
    let me = std::process::id().to_string();
    if std::fs::read_to_string(&pid_file)
        .is_ok_and(|r| r.split_whitespace().next() == Some(me.as_str()))
    {
        let _ = std::fs::remove_file(&pid_file);
    }
    Ok(())
}

/// Final text from continuous decoding: one more tick over the whole
/// recording, forcing what the preview already settled. None means use a
/// full transcription instead.
pub(super) async fn finish_continuous(
    config: &Config,
    audio_file: &std::path::Path,
    owner: Option<&str>,
    server_url: &str,
    model: Option<String>,
    language: Option<&str>,
) -> Option<String> {
    use ears::continuous::{samples, ContinuousDecoder, DecoderState};
    const DEADLINE: Duration = Duration::from_secs(4);

    let state_file = ghost_continuous_state_file(config);
    let state: Option<DecoderState> = std::fs::read_to_string(&state_file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let _ = std::fs::remove_file(&state_file);
    // Only this recording's settled text may be forced. Without it (no
    // preview tick landed, or continuous decoding stopped working) a full
    // transcription is just as fast.
    let state =
        state.filter(|s| !s.unsupported && owner.is_some() && s.owner.as_deref() == owner)?;
    let bytes = tokio::fs::read(audio_file).await.ok()?;
    let all = samples(ears::ghost::growing_wav_payload(&bytes)?);
    // Only the last segment is decoded; earlier ones are already text.
    let segment = all.get(state.offset..)?;
    if segment.len() * 2 > MAX_CONTINUOUS_BYTES {
        return None; // past the server's context
    }
    let prefix = state.prefix.clone();
    let started = std::time::Instant::now();
    let mut decoder = ContinuousDecoder::new(server_url, config.api_key.clone(), model)
        .with_language(language)
        .with_context(config.prompt.clone())
        .resume(state);
    match decoder.step(segment, true, DEADLINE).await {
        Ok(text) if !text.is_empty() => {
            tracing::info!("Continuous final in {:?}", started.elapsed());
            Some(ears::continuous::join_segments(&prefix, &text))
        }
        // A last segment of silence still leaves the earlier ones.
        Ok(_) if !prefix.is_empty() => Some(prefix),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("Continuous final failed, transcribing in full: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_process_identity_rejects_recycled_pid() {
        let me = std::process::id() as i32;
        let start = proc_start_time(me).expect("own start time");
        assert!(same_process(me, start));
        // Same PID, different start: a recycled PID is not the recorder.
        assert!(!same_process(me, start + 1));
        assert!(!same_process(0, start));
        assert!(!same_process(-1, start));
    }

    #[test]
    fn test_preview_stops_when_its_recorder_is_replaced() {
        // A preview is bound to one recorder: once that process is gone, a
        // new recording (whatever its PID) does not keep the preview alive.
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let start = proc_start_time(pid).unwrap();
        assert!(same_process(pid, start));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!same_process(pid, start));
    }
}
