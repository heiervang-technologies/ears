//! Chant mode in the engine ([`crate::chant`]): the chant is heard while it
//! goes on, a window at a time, and the action fires mid-chant.
//!
//! Chanting is a sub-state of command mode
//! ([`crate::vad_mode::CommandState::chant`]): leaving command mode, by any
//! way [`StreamingEngine::set_mode`] knows, drops it and its files.
//!
//! While command mode's text anchors are still loading (ears#273) a chant
//! goes by its words alone: no audio or meaning is sent to the embedding
//! server, so its hearings never queue behind the load batches and exact
//! phrasings still fire at the usual pace.

use super::*;
use crate::chant::{self as ch, ChantState};
use crate::vad_mode::CommandState;

/// A mid-chant hearing waits this long for the words.
const CHANT_ASR_BUDGET: std::time::Duration = std::time::Duration::from_millis(900);
/// Voice gone this long without a segment (it was dropped): the breath is over.
const BREATH_GONE: std::time::Duration = std::time::Duration::from_millis(2500);

impl StreamingEngine {
    /// Whether chant mode is on.
    pub fn chanting(&self) -> bool {
        self.mode.is_chanting()
    }

    fn chant_mut(&mut self) -> Option<&mut ChantState> {
        self.mode.command_state_mut().and_then(|s| s.chant.as_mut())
    }

    /// Chanting on or off (command mode stays on); `None` toggles. Only in
    /// command mode: outside it there is no chant to switch.
    pub(super) fn set_chant(&mut self, on: Option<bool>) {
        let Some(paths) = self.command_paths.clone() else {
            return;
        };
        let Some(state) = self.mode.command_state_mut() else {
            return;
        };
        let on = on.unwrap_or(state.chant.is_none());
        if on && state.chant.is_none() {
            info!("Chant mode on: say it again until it lands");
            state.chant = Some(ChantState::new(&paths.state_dir));
            self.chant_show(false, "", None, false);
        } else if !on && state.chant.take().is_some() {
            info!("Chant mode off");
        }
    }

    /// Write what the light shows.
    fn chant_show(&mut self, live: bool, heard: &str, fired: Option<String>, miss: bool) {
        let Some(CommandState {
            mode,
            chant: Some(state),
        }) = self.mode.command_state_mut()
        else {
            return;
        };
        state.seq += 1;
        let line = ch::Line {
            seq: state.seq,
            live,
            meters: ch::meters(&mode.router, &state.meter),
            heard: heard.to_string(),
            fired,
            miss,
        };
        let tmp = state.file.with_extension("json.tmp");
        if let Ok(bytes) = serde_json::to_vec(&line) {
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &state.file);
            }
        }
    }

    /// The embedder for a chant hearing: none while the text anchors load
    /// (the chant then goes by its words), or outside command mode.
    fn chant_embedder(&mut self) -> Option<crate::command_mode::Embedder> {
        self.mode
            .command_mut()
            .and_then(|m| m.ready().then(|| m.embedder.clone()))
    }

    /// After each chunk of audio: hear the chant so far every [`ch::TICK`],
    /// and act on what came back.
    pub(super) async fn chant_listen(&mut self, chunk: usize) {
        let voice = self.vad_detector.is_probably_speaking() || self.vad_detector.is_speaking();
        let rate = crate::pcm::SAMPLE_RATE;
        let Some(state) = self.chant_mut() else {
            return;
        };
        if voice {
            state.speech += chunk;
            state.last_voice = Instant::now();
        } else if state.speech > 0 && state.last_voice.elapsed() > BREATH_GONE {
            state.breath_end();
        }
        if state.pending.as_ref().is_some_and(|j| j.is_finished()) {
            if let Some(Ok((audio, words))) = match state.pending.take() {
                Some(job) => Some(job.await),
                None => None,
            } {
                self.chant_heard(audio, &words, 1.0, true).await;
            }
        }
        let Some(state) = self.chant_mut() else {
            return;
        };
        let due = state.last_tick.is_none_or(|t| t.elapsed() >= ch::TICK);
        let long_enough = state.speech as f32 >= ch::MIN_SPEECH.as_secs_f32() * rate as f32;
        if !voice || state.fired || state.pending.is_some() || !due || !long_enough {
            return;
        }
        let window = (ch::WINDOW.as_secs_f32() * rate as f32) as usize;
        let speech = state.speech;
        let embedder = self.chant_embedder();
        let context = self.asr_context();
        let samples = self.audio_buffer.read_recent(window.min(speech));
        let job = tokio::spawn(ch::hear(
            embedder,
            self.whisper_client.clone(),
            self.temp_dir.clone(),
            samples,
            CHANT_ASR_BUDGET,
            context,
        ));
        if let Some(state) = self.chant_mut() {
            state.last_tick = Some(Instant::now());
            state.pending = Some(job);
        } else {
            job.abort();
        }
    }

    /// Add one hearing to the meters (`scale`: how much it counts) and fire
    /// the action whose meter is full. Returns whether one fired.
    async fn chant_heard(
        &mut self,
        audio: Option<Vec<f32>>,
        words: &str,
        scale: f32,
        live: bool,
    ) -> bool {
        let Some(mode) = self.mode.command_mut() else {
            return false;
        };
        let hearing = mode.chant_hear(audio.as_deref(), words).await;
        let Some(CommandState {
            mode,
            chant: Some(state),
        }) = self.mode.command_state_mut()
        else {
            return false;
        };
        if state.fired {
            return false;
        }
        let evidence = ch::evidence(
            &mode.router,
            hearing.named.as_ref().map(|n| n.0),
            hearing.audio.as_ref(),
            hearing.meaning.as_ref(),
        );
        if let Some((i, Some(rest))) = &hearing.named {
            state.rest.insert(*i, rest.clone());
        }
        if !hearing.heard.is_empty() && !state.readings.contains(&hearing.heard) {
            state.readings.push(hearing.heard.clone());
        }
        state.meter.add(Instant::now(), &evidence, scale);
        debug!(
            "Chant: heard {:?}, meters {:?}",
            hearing.heard,
            state.meter.ranked()
        );
        let Some(i) = state.meter.ready() else {
            self.chant_show(live, &hearing.heard, None, false);
            return false;
        };
        state.fired = true;
        state.meter.clear();
        if let Some(job) = state.pending.take() {
            job.abort();
        }
        let rest = state.rest.remove(&i);
        state.rest.clear();
        let readings = std::mem::take(&mut state.readings);
        let name = mode.router.actions[i].name.clone();
        let effect = mode.effect(i, rest);
        // The breath's other readings are mining evidence (`alts`).
        if let Some(journal) = &self.cs_journal {
            let by_words = hearing.named.as_ref().is_some_and(|n| n.0 == i);
            journal.chant(Some(&name), &hearing.heard, by_words, &readings);
        }
        info!("Chant: {:?} fires {} ({:?})", hearing.heard, name, effect);
        self.chant_show(live, &hearing.heard, Some(name.clone()), false);
        self.subtitle_fired(&hearing.heard, &name);
        self.fire_effect(&name, effect).await;
        true
    }

    /// The breath ended (a whole segment): heard once more in full unless
    /// it already fired; nothing fired is a miss, kept for `agent`.
    pub(super) async fn chant_segment(&mut self, samples: &[f32]) {
        let Some(state) = self.chant_mut() else {
            return;
        };
        if std::mem::take(&mut state.fired) {
            state.breath_end();
            self.discard("the chant already fired");
            return;
        }
        // The hearings so far stay for the journal: the whole breath is
        // one more reading of them.
        let readings = std::mem::take(&mut state.readings);
        state.breath_end();
        state.readings = readings;
        let embedder = self.chant_embedder();
        let context = self.asr_context();
        let (audio, words) = ch::hear(
            embedder,
            self.whisper_client.clone(),
            self.temp_dir.clone(),
            samples.to_vec(),
            COMMAND_ASR_BUDGET,
            context,
        )
        .await;
        if self.chant_heard(audio, &words, ch::BREATH, false).await {
            if let Some(state) = self.chant_mut() {
                state.fired = false;
            }
            return;
        }
        let heard = ch::units(&words).into_iter().next().unwrap_or_default();
        let nearest = match self.mode.command_state_mut() {
            Some(CommandState {
                mode,
                chant: Some(state),
            }) => {
                let nearest = state
                    .meter
                    .ranked()
                    .first()
                    .map(|(i, _)| mode.router.actions[*i].name.clone());
                mode.missed(&heard);
                let readings = std::mem::take(&mut state.readings);
                if let Some(journal) = &self.cs_journal {
                    journal.chant(None, &heard, false, &readings);
                }
                nearest
            }
            // The breath's firing (or something else) left chant mode.
            _ => return,
        };
        info!("Chant: {:?} fired nothing (nearest {:?})", heard, nearest);
        self.chant_show(false, &heard, None, true);
        self.discard("no action is a confident match");
        self.subtitle_missed(&heard, nearest.as_deref());
    }

    /// A breath that never became a segment (dropped as not speech).
    pub(super) fn chant_breath_dropped(&mut self) {
        if let Some(state) = self.chant_mut() {
            state.breath_end();
        }
    }
}
