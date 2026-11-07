//! Chant mode: command mode for saying it again until it lands.
//!
//! Saying "chant" (in any mode) turns command mode on with chanting: the
//! light stays violet and a meter per action appears above the subtitles.
//! While you chant ("close close close", "open terminal open terminal"),
//! the last couple of seconds of audio are heard every third of a second:
//! embedded, and transcribed with the repeats folded to one ([`units`]).
//! Each hearing adds to the meter of the action it points to; meters fade
//! with time ([`HALF_LIFE`]). An action fires as soon as its meter is full
//! and well ahead of the next ([`Meter::ready`]), mid-chant, without waiting
//! for a pause. The rest of that breath is then ignored, so one chant fires
//! once. A breath that ends without firing is a miss: its words are kept for
//! the escape hatch, the built-in `agent` action ("agent", "send to agent"),
//! which hands them to an agent when no command is right for what you mean.
//!
//! "chant" again goes back to plain command mode; "command" leaves both.

use crate::command_mode::{plain, Decision, Router};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Marker in the runtime dir while chanting (read by the light).
pub const MARKER: &str = "vad.chant";
/// The meters, for the light: see [`Line`].
pub const FILE: &str = "chant.json";

/// Spoken alone, these switch chant mode on (from dictation, standby or
/// command mode) or, in it, back to plain command mode.
pub const PHRASES: &[&str] = &["chant", "chants", "chant mode", "chanting"];

/// How often the chant is heard while it goes on.
pub const TICK: Duration = Duration::from_millis(330);
/// The last this-much of the chant is heard each time.
pub const WINDOW: Duration = Duration::from_millis(2000);
/// Speech shorter than this is not heard mid-chant yet.
pub const MIN_SPEECH: Duration = Duration::from_millis(600);
/// A meter halves in this time.
pub const HALF_LIFE: Duration = Duration::from_millis(2500);
/// A full meter.
pub const FULL: f32 = 1.0;
/// The leader fires only this many times ahead of the runner-up.
pub const LEAD: f32 = 2.0;

/// What one hearing adds: an exact phrasing among the folded words...
const WORDS: f32 = 0.6;
/// ...the audio's vote, and the folded words' meaning, each at most this.
const AUDIO: f32 = 0.35;
const MEANING: f32 = 0.35;
/// The whole breath, heard once it ends, counts this much more.
pub const BREATH: f32 = 1.5;

/// The phrase a chant repeats, best first: the word run that occurs most
/// often without overlapping itself, weighted by its length, shorter first
/// on a tie. A window cut mid-phrase repeats a rotation too ("terminal open
/// terminal open"), so every equally good run is returned; the whole text
/// comes last. "" for no words.
pub fn units(text: &str) -> Vec<String> {
    let said = plain(text);
    let words: Vec<&str> = said.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut best: Vec<(usize, usize, String)> = Vec::new(); // (score, len, unit)
    for len in 1..=words.len() / 2 {
        let mut counts: HashMap<&[&str], (usize, usize)> = HashMap::new(); // (count, end of last)
        for start in 0..=words.len() - len {
            let gram = &words[start..start + len];
            let entry = counts.entry(gram).or_insert((0, 0));
            if entry.0 == 0 || start >= entry.1 {
                *entry = (entry.0 + 1, start + len);
            }
        }
        for (gram, (count, _)) in counts {
            if count >= 2 {
                best.push((count * len, len, gram.join(" ")));
            }
        }
    }
    best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let top = best.first().map_or(0, |b| b.0);
    let mut out: Vec<String> = best
        .into_iter()
        .take_while(|b| b.0 == top)
        .map(|b| b.2)
        .collect();
    if !out.contains(&said) {
        out.push(said);
    }
    out
}

/// The action the folded words name, and the words after a `rest` action's
/// phrasing ("agent open my mail" -> agent, "open my mail").
pub fn by_words(router: &Router, units: &[String]) -> Option<(usize, Option<String>)> {
    units.iter().find_map(|u| {
        if let Some((i, rest)) = router.by_rest(u) {
            return Some((i, Some(rest)));
        }
        router
            .by_words(u)
            .filter(|&i| !router.actions[i].rest)
            .map(|i| (i, None))
    })
}

/// What one hearing of the chant adds to each action's meter.
pub fn evidence(
    router: &Router,
    words: Option<usize>,
    audio: Option<&Decision>,
    meaning: Option<&Decision>,
) -> Vec<(usize, f32)> {
    let mut out = Vec::new();
    if let Some(i) = words {
        out.push((i, WORDS));
    }
    for (decision, weight) in [(audio, AUDIO), (meaning, MEANING)] {
        let Some(d) = decision else { continue };
        let Some((name, share)) = d.votes.first() else {
            continue;
        };
        let Some(i) = router.index(name) else {
            continue;
        };
        let action = &router.actions[i];
        // Chatter, and actions that go by their words only, gain nothing
        // from how it sounds or what it means.
        if action.exact || action.effect == crate::command_mode::Effect::Nothing {
            continue;
        }
        // Half the firing floor counts for nothing, the floor in full.
        let half = router.floor / 2.0;
        let sure = ((d.nearest - half) / half.max(1e-3)).clamp(0.0, 1.0);
        let clear = (d.margin / router.margin.max(1e-3)).clamp(0.0, 1.0);
        let add = weight * share * sure * (0.5 + 0.5 * clear);
        if add > 0.0 {
            out.push((i, add));
        }
    }
    out
}

/// Each action's meter, fading with time.
#[derive(Debug, Clone, Default)]
pub struct Meter {
    levels: HashMap<usize, f32>,
    at: Option<Instant>,
}

impl Meter {
    fn fade_to(&mut self, now: Instant) {
        if let Some(at) = self.at {
            let dt = now.saturating_duration_since(at).as_secs_f32();
            let k = 0.5f32.powf(dt / HALF_LIFE.as_secs_f32());
            self.levels.values_mut().for_each(|l| *l *= k);
            self.levels.retain(|_, l| *l > 0.01);
        }
        self.at = Some(now);
    }

    /// Fade to `now`, then add `scale` times the evidence.
    pub fn add(&mut self, now: Instant, evidence: &[(usize, f32)], scale: f32) {
        self.fade_to(now);
        for &(i, add) in evidence {
            *self.levels.entry(i).or_default() += add * scale;
        }
    }

    /// Levels, highest first.
    pub fn ranked(&self) -> Vec<(usize, f32)> {
        let mut r: Vec<(usize, f32)> = self.levels.iter().map(|(&i, &l)| (i, l)).collect();
        r.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        r
    }

    /// The action whose meter is full and [`LEAD`] times ahead.
    pub fn ready(&self) -> Option<usize> {
        let r = self.ranked();
        let (lead, level) = *r.first()?;
        let next = r.get(1).map_or(0.0, |x| x.1);
        (level >= FULL && level >= LEAD * next).then_some(lead)
    }

    pub fn clear(&mut self) {
        self.levels.clear();
    }
}

/// What the light shows (`chant.json`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Line {
    pub seq: u64,
    /// Chanting right now.
    pub live: bool,
    /// The leading actions and how full their meters are (0 to 1).
    pub meters: Vec<(String, f32)>,
    /// What the chant sounds like it says (the folded words).
    pub heard: String,
    /// The action that just fired.
    pub fired: Option<String>,
    /// The breath ended with nothing fired.
    pub miss: bool,
}

/// How many meters the light shows.
pub const SHOWN: usize = 3;

pub fn meters(router: &Router, meter: &Meter) -> Vec<(String, f32)> {
    meter
        .ranked()
        .into_iter()
        .take(SHOWN)
        .map(|(i, l)| (router.actions[i].name.clone(), (l / FULL).min(1.0)))
        .collect()
}

/// Audio embedding of a window (if the server answered), and its words.
pub type Heard = (Option<Vec<f32>>, String);

/// Chanting: a sub-state of command mode
/// ([`crate::vad_mode::CommandState::chant`]), so it ends when command mode
/// does. While it lives, [`MARKER`] is up; dropping it takes the marker and
/// [`FILE`] down and stops the hearing in flight.
pub struct ChantState {
    pub(crate) meter: Meter,
    /// Speech in this breath so far, in samples.
    pub(crate) speech: usize,
    pub(crate) last_voice: Instant,
    pub(crate) last_tick: Option<Instant>,
    /// Fired in this breath: the rest of it is ignored.
    pub(crate) fired: bool,
    /// The hearing in flight.
    pub(crate) pending: Option<tokio::task::JoinHandle<Heard>>,
    /// The words after a `rest` action's phrasing, latest per action.
    pub(crate) rest: HashMap<usize, String>,
    /// This breath's folded words, hearing by hearing (distinct): the
    /// other readings of a firing, journaled as `alts` for `ears cs mine`.
    pub(crate) readings: Vec<String>,
    pub(crate) seq: u64,
    /// [`FILE`] in the state dir.
    pub(crate) file: std::path::PathBuf,
    /// [`MARKER`] in the state dir.
    marker: std::path::PathBuf,
}

impl ChantState {
    /// Start chanting: puts [`MARKER`] up in `state_dir`.
    pub fn new(state_dir: &std::path::Path) -> Self {
        let marker = state_dir.join(MARKER);
        if let Err(e) = std::fs::write(&marker, "") {
            tracing::warn!("Cannot write the chant marker: {}", e);
        }
        Self {
            meter: Meter::default(),
            speech: 0,
            last_voice: Instant::now(),
            last_tick: None,
            fired: false,
            pending: None,
            rest: HashMap::new(),
            readings: Vec::new(),
            seq: 0,
            file: state_dir.join(FILE),
            marker,
        }
    }

    /// A breath is over: the next one may fire again.
    pub fn breath_end(&mut self) {
        if let Some(job) = self.pending.take() {
            job.abort();
        }
        self.speech = 0;
        self.last_tick = None;
        self.fired = false;
        self.readings.clear();
    }
}

impl Drop for ChantState {
    fn drop(&mut self) {
        if let Some(job) = self.pending.take() {
            job.abort();
        }
        let _ = std::fs::remove_file(&self.marker);
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Hear one window of the chant: its audio embedded, and its words.
/// Without an `embedder` (command mode's text anchors are still loading,
/// see [`crate::command_mode::CommandMode::ready`]) it is heard by its words
/// alone, so it never queues behind the load on the embedding server.
/// `context` is the ASR context of command mode ([`crate::cs::context`]).
pub async fn hear(
    embedder: Option<crate::command_mode::Embedder>,
    whisper: std::sync::Arc<crate::WhisperClient>,
    dir: std::path::PathBuf,
    samples: Vec<f32>,
    budget: std::time::Duration,
    context: Option<String>,
) -> Heard {
    let pcm: Vec<i16> = samples.iter().map(|s| crate::pcm::to_i16(*s)).collect();
    let wav = crate::pcm::wav_bytes(&pcm);
    let file = tempfile::Builder::new()
        .prefix("chant_")
        .suffix(".wav")
        .tempfile_in(&dir)
        .ok()
        .and_then(|f| {
            std::fs::write(f.path(), &wav)
                .ok()
                .map(|_| f.into_temp_path())
        });
    let asr = async {
        match &file {
            Some(path) => tokio::time::timeout(
                budget,
                whisper.transcribe_with_context(path, false, context.as_deref()),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default(),
            None => String::new(),
        }
    };
    let audio = async {
        match &embedder {
            Some(e) => e.audio(&wav).await.ok(),
            None => None,
        }
    };
    tokio::join!(audio, asr)
}

/// `ears chant-file`: replay a recording as if it were chanted, hearing it
/// every [`TICK`] as chant mode would (speech assumed from the start), then
/// once whole. Prints the meters after each hearing; stops at a firing.
pub async fn replay(
    mode: &mut crate::command_mode::CommandMode,
    whisper: std::sync::Arc<crate::WhisperClient>,
    samples: &[f32],
) -> Option<String> {
    let rate = crate::pcm::SAMPLE_RATE as f32;
    let dir = std::env::temp_dir();
    let start = Instant::now();
    let mut meter = Meter::default();
    let show = |router: &Router, meter: &Meter| {
        meters(router, meter)
            .iter()
            .map(|(n, l)| format!("{n} {:.0}%", l * 100.0))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut t = MIN_SPEECH;
    let total = Duration::from_secs_f32(samples.len() as f32 / rate);
    let heard_at = |t: Duration, scale: f32| {
        let end = ((t.as_secs_f32() * rate) as usize).min(samples.len());
        let from = if scale > 1.0 {
            0
        } else {
            end.saturating_sub((WINDOW.as_secs_f32() * rate) as usize)
        };
        (samples[from..end].to_vec(), scale)
    };
    let mut steps = Vec::new();
    while t <= total {
        steps.push((t, heard_at(t, 1.0)));
        t += TICK;
    }
    steps.push((total, heard_at(total, BREATH)));
    for (t, (window, scale)) in steps {
        let asked = Instant::now();
        let (audio, words) = hear(
            Some(mode.embedder.clone()),
            whisper.clone(),
            dir.clone(),
            window,
            Duration::from_millis(1500),
            None,
        )
        .await;
        let took = asked.elapsed();
        let h = mode.chant_hear(audio.as_deref(), &words).await;
        let e = evidence(
            &mode.router,
            h.named.as_ref().map(|n| n.0),
            h.audio.as_ref(),
            h.meaning.as_ref(),
        );
        meter.add(start + t, &e, scale);
        println!(
            "{:5.2} s{} {:>5} ms  {:<28} {}",
            t.as_secs_f32(),
            if scale > 1.0 { " whole" } else { "      " },
            took.as_millis(),
            format!("{:?}", h.heard),
            show(&mode.router, &meter)
        );
        if let Some(i) = meter.ready() {
            let name = mode.router.actions[i].name.clone();
            println!("fires {name}");
            return Some(name);
        }
    }
    println!("fires nothing");
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_mode::{normalized, parse_actions};

    #[test]
    fn the_chant_state_owns_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = ChantState::new(dir.path());
        assert!(dir.path().join(MARKER).exists());
        std::fs::write(dir.path().join(FILE), "{}").unwrap();
        drop(state);
        assert!(!dir.path().join(MARKER).exists());
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn a_chant_folds_to_what_it_repeats() {
        assert_eq!(units("close close close close")[0], "close");
        assert_eq!(units("Open terminal, open terminal!")[0], "open terminal");
        assert_eq!(units("google google google")[0], "google");
        assert_eq!(
            units("I'm feeling lucky I'm feeling lucky!!!")[0],
            "i'm feeling lucky"
        );
        // a misheard repeat still leaves the phrase ahead
        assert_eq!(
            units("open terminal open terminals open terminal")[0],
            "open terminal"
        );
        // a window cut mid-phrase: the rotation is offered too
        let cut = units("terminal open terminal open terminal");
        assert!(cut.contains(&"open terminal".to_string()), "{cut:?}");
        // said once, it is just the words
        assert_eq!(units("next window"), vec!["next window".to_string()]);
        assert!(units("  ").is_empty());
    }

    fn router() -> Router {
        let (_, actions) = parse_actions(
            "[[action]]\nname = \"close\"\nkeys = \"enter\"\ntext = [\"close\", \"close window\"]\n\
             [[action]]\nname = \"terminal\"\nrun = [\"true\"]\ntext = [\"open terminal\"]\n\
             [[action]]\nname = \"layout\"\nrun = [\"true\"]\nexact = true\n",
        )
        .unwrap();
        let mut r = Router::new(actions, 2.0, 0.4, 3);
        for (n, axis) in [("close", 0), ("terminal", 1), ("none", 2), ("leave", 3)] {
            let mut v = vec![0.0; 5];
            v[axis] = 1.0;
            let i = r.index(n).unwrap();
            r.add_anchor(i, normalized(v), false);
        }
        r
    }

    #[test]
    fn folded_words_name_the_action() {
        let r = router();
        let close = r.index("close");
        assert_eq!(
            by_words(&r, &units("close close close")).map(|w| w.0),
            close
        );
        assert_eq!(
            by_words(&r, &units("terminal open terminal open terminal")).map(|w| w.0),
            r.index("terminal")
        );
        assert_eq!(
            by_words(&r, &units("agent open my mail")),
            Some((r.index("agent").unwrap(), Some("open my mail".into())))
        );
        assert_eq!(by_words(&r, &units("banana banana")), None);
    }

    #[test]
    fn exact_words_fill_the_meter_in_two_hearings() {
        let r = router();
        let close = r.index("close").unwrap();
        let mut m = Meter::default();
        let t = Instant::now();
        m.add(t, &evidence(&r, Some(close), None, None), 1.0);
        assert_eq!(m.ready(), None, "one hearing is not enough");
        m.add(t + TICK, &evidence(&r, Some(close), None, None), 1.0);
        assert_eq!(m.ready(), Some(close));
    }

    #[test]
    fn a_split_chant_fires_nothing_and_meters_fade() {
        let r = router();
        let (close, term) = (r.index("close").unwrap(), r.index("terminal").unwrap());
        let mut m = Meter::default();
        let t = Instant::now();
        for k in 0..4 {
            let i = if k % 2 == 0 { close } else { term };
            m.add(t + TICK * k, &evidence(&r, Some(i), None, None), 1.0);
        }
        assert_eq!(m.ready(), None, "two actions neck and neck");
        m.add(t + TICK * 4 + HALF_LIFE * 4, &[], 1.0);
        assert!(m.ranked().iter().all(|x| x.1 < 0.1), "{:?}", m.ranked());
    }

    #[test]
    fn sound_and_meaning_add_up_but_never_for_exact_actions() {
        let r = router();
        let close = r.index("close").unwrap();
        let near_close = r.decide(&normalized(vec![1.0, 0.05, 0.0, 0.0, 0.0]));
        let e = evidence(&r, None, Some(&near_close), Some(&near_close));
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|x| x.0 == close && x.1 > 0.0));
        let mut m = Meter::default();
        let t = Instant::now();
        for k in 0..6 {
            m.add(t + TICK * k, &e, 1.0);
        }
        assert_eq!(m.ready(), Some(close), "{:?}", m.ranked());

        let layout = r.index("layout").unwrap();
        let by_sound = Decision {
            votes: vec![("layout".into(), 1.0)],
            nearest: 5.0,
            margin: 5.0,
            fire: None,
            fired: None,
        };
        assert!(evidence(&r, None, Some(&by_sound), None).is_empty());
        assert_eq!(
            evidence(&r, Some(layout), None, None),
            vec![(layout, WORDS)]
        );
    }
}
