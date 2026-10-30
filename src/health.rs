//! Desktop VAD health, independent of the audio/transcription executor.
//!
//! The atomically replaced sidecar is additive: the legacy state file and IPC
//! events retain their formats. No audio or transcript text is retained here.
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const HEALTH_FILE: &str = "vad-health.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Listening,
    Detecting,
    Saving,
    Transcribing,
    Typing,
    Stopped,
}

/// Whether the transcription server can be reached, as last observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrState {
    /// The live stream is up, or (no stream in use) the server answers.
    Ok,
    /// The live stream is down or missing, but plain HTTP requests work.
    Degraded,
    /// The last contact with the server failed and none succeeded since.
    Down,
    /// No contact yet.
    Unknown,
}

/// How the server was last reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrPath {
    Stream,
    Http,
}

/// The `asr` object of `vad-health.json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AsrHealth {
    pub state: AsrState,
    pub path: Option<AsrPath>,
    /// Last successful contact, on the same clock as `updated_monotonic_ms`.
    pub last_ok_ms: Option<u64>,
    pub last_error: Option<String>,
}

/// What the live stream connection (ears stream protocol) is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamStatus {
    /// Live decoding does not use the stream.
    #[default]
    Unused,
    Connecting,
    Up,
    /// Lost, refused, or the server has no stream endpoint.
    Down,
}

/// Observations behind [`AsrHealth`].
#[derive(Clone, Debug, Default)]
struct AsrTrack {
    stream: StreamStatus,
    /// The stream was up or down at least once (a first connect in progress
    /// does not count as degraded).
    stream_settled: bool,
    last_ok_ms: Option<u64>,
    last_ok_path: Option<AsrPath>,
    last_error: Option<String>,
    failing: bool,
}

impl AsrTrack {
    fn report(&self) -> AsrHealth {
        let stream_down = match self.stream {
            StreamStatus::Unused => false,
            StreamStatus::Connecting => self.stream_settled,
            StreamStatus::Up => false,
            StreamStatus::Down => true,
        };
        let (state, path) = if self.stream == StreamStatus::Up {
            (AsrState::Ok, Some(AsrPath::Stream))
        } else if self.failing {
            (AsrState::Down, None)
        } else if self.last_ok_ms.is_none() {
            (AsrState::Unknown, None)
        } else if stream_down {
            (AsrState::Degraded, Some(AsrPath::Http))
        } else {
            (AsrState::Ok, self.last_ok_path)
        };
        AsrHealth {
            state,
            path,
            last_ok_ms: self.last_ok_ms,
            last_error: self.last_error.clone(),
        }
    }
}

/// While the live stream is not up, the server is probed this often.
pub const ASR_PROBE_INTERVAL: Duration = Duration::from_secs(10);

/// A probe slower than this counts as a failure.
pub const ASR_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// `GET /health` (vLLM, whisper.cpp), else `GET /v1/models` (cloud APIs).
/// A connection error or a server error fails at once.
pub async fn probe_asr(
    client: &reqwest::Client,
    server_url: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let base = server_url.trim_end_matches('/');
    let mut last = String::new();
    for path in ["/health", "/v1/models"] {
        let mut request = client.get(format!("{base}{path}"));
        if let Some(key) = api_key {
            request = request.bearer_auth(key);
        }
        match request.send().await {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) if r.status().is_server_error() => return Err(format!("{path}: {}", r.status())),
            Ok(r) => last = format!("{path}: {}", r.status()),
            Err(e) if e.is_timeout() => return Err(format!("{path}: timed out")),
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
    Err(last)
}

/// Background probe of the transcription server; stops when dropped.
pub struct AsrProbe(tokio::task::JoinHandle<()>);

impl Drop for AsrProbe {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// All times are CLOCK_MONOTONIC milliseconds, comparable by local consumers.
pub fn monotonic_ms() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // CLOCK_MONOTONIC is supported on the Linux desktop targeted by Ears.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthSnapshot {
    pub version: u8,
    pub pid: u32,
    pub process_start_ticks: String,
    pub boot_id: String,
    pub session_id: String,
    pub session_kind: &'static str,
    pub device: String,
    pub updated_monotonic_ms: u64,
    pub stage: Stage,
    pub stage_since_ms: u64,
    pub capture_last_ms: Option<u64>,
    pub detector_last_ms: Option<u64>,
    pub captured_samples: u64,
    pub processed_frames: u64,
    pub queue_chunks: usize,
    pub audio_backlog_ms: u64,
    pub rms: f32,
    pub peak: f32,
    pub probability: f32,
    pub threshold: f32,
    pub candidate_frames: usize,
    pub rejected_candidates: u64,
    pub last_rejected_frames: usize,
    pub last_rejection_probability: f32,
    /// Highest probability the last rejected candidate reached.
    pub last_rejection_peak: f32,
    /// Speech-level gain currently applied before detection (1.0 = none).
    pub gain: f32,
    /// Mean probability over the voiced span of the last segment.
    pub last_segment_mean_probability: f32,
    /// Segments dropped as too unlikely to be speech.
    pub dropped_segments: u64,
    pub speaking: bool,
    pub capture_error: Option<String>,
    pub typing_paused: bool,
    /// Background level between utterances (dBFS, smoothed; raw input,
    /// before gain).
    pub noise_dbfs: f32,
    /// Speech level while speaking (dBFS, smoothed; raw input).
    pub speech_dbfs: f32,
    pub clipped_samples: u64,
    /// Microphone quality problem, if any. `ears mic-check` explains more.
    pub mic_warning: Option<&'static str>,
    /// Transcription server reachability.
    pub asr: AsrHealth,
    #[serde(skip)]
    levels: MicLevels,
    #[serde(skip)]
    asr_track: AsrTrack,
}

/// Smoothed input levels behind [`HealthSnapshot::mic_warning`].
#[derive(Clone, Debug, Default)]
struct MicLevels {
    noise_power: f32,
    speech_power: f32,
    noise_chunks: u32,
    speech_chunks: u32,
    /// Smoothed fraction of clipped samples.
    clip_rate: f32,
}

/// Capture chunks (100 ms) of each kind before their level is judged.
const MIC_SETTLE_CHUNKS: u32 = 20;

impl MicLevels {
    fn observe(&mut self, rms: f32, clipped: f32, speaking: bool) {
        let power = rms * rms;
        if speaking {
            let alpha = if self.speech_chunks == 0 { 1.0 } else { 0.1 };
            self.speech_power += alpha * (power - self.speech_power);
            self.speech_chunks = self.speech_chunks.saturating_add(1);
        } else {
            // A floor tracker: falls fast, rises slowly, so speech onsets the
            // detector has not flagged yet barely lift it.
            let alpha = match self.noise_chunks {
                0 => 1.0,
                _ if power < self.noise_power => 0.2,
                _ => 0.02,
            };
            self.noise_power += alpha * (power - self.noise_power);
            self.noise_chunks = self.noise_chunks.saturating_add(1);
        }
        self.clip_rate += 0.05 * (clipped - self.clip_rate);
    }

    fn noise_dbfs(&self) -> f32 {
        crate::pcm::dbfs(self.noise_power.sqrt())
    }

    fn speech_dbfs(&self) -> f32 {
        crate::pcm::dbfs(self.speech_power.sqrt())
    }

    /// Same thresholds as `ears mic-check`, judged on live audio.
    fn warning(&self) -> Option<&'static str> {
        use crate::mic_check as mc;
        let noise_known = self.noise_chunks >= MIC_SETTLE_CHUNKS;
        let speech_known = self.speech_chunks >= MIC_SETTLE_CHUNKS;
        if noise_known && self.noise_dbfs() <= mc::DEAD_DBFS && self.speech_chunks == 0 {
            return Some("mic silent");
        }
        if self.clip_rate > mc::MAX_CLIPPED {
            return Some("mic clipping");
        }
        if speech_known && self.speech_dbfs() < mc::QUIET_DBFS {
            return Some("speech too quiet");
        }
        if noise_known && self.noise_dbfs() > mc::NOISY_DBFS {
            return Some("noisy background");
        }
        None
    }
}

impl HealthSnapshot {
    /// Diagnose progress, not the amount of sound: zero-valued audio is alive.
    pub fn problem(&self, now: u64) -> Option<&'static str> {
        if self.stage == Stage::Stopped {
            return Some("pipeline stopped");
        }
        if self.capture_error.is_some() {
            return Some("capture stopped");
        }
        if self
            .capture_last_ms
            .is_some_and(|t| now.saturating_sub(t) > 3000)
            || (self.capture_last_ms.is_none() && now.saturating_sub(self.stage_since_ms) > 3000)
        {
            return Some("no audio arriving");
        }
        if self.typing_paused {
            return Some("typing paused after output error");
        }
        if self.asr.state == AsrState::Down {
            return Some("transcription server unreachable");
        }
        let slow_after_ms = if self.stage == Stage::Transcribing {
            20_000
        } else {
            5_000
        };
        if self.stage != Stage::Listening && now.saturating_sub(self.stage_since_ms) > slow_after_ms
        {
            return Some("slow pipeline stage");
        }
        if self.stage == Stage::Listening
            && now.saturating_sub(self.detector_last_ms.unwrap_or(self.stage_since_ms)) > 3000
        {
            return Some("voice detection stalled");
        }
        if matches!(self.stage, Stage::Listening | Stage::Detecting) && self.audio_backlog_ms > 2000
        {
            return Some("audio backlog");
        }
        None
    }
}

#[derive(Clone)]
pub struct PipelineHealth(Arc<Mutex<HealthSnapshot>>);

impl PipelineHealth {
    fn update(&self, f: impl FnOnce(&mut HealthSnapshot)) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut state);
    }

    pub fn device(&self, device: &str) {
        self.update(|s| s.device = device.to_owned());
    }

    pub fn captured(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let peak = crate::pcm::peak(samples);
        let rms = crate::pcm::rms(samples);
        let clipped = crate::pcm::clipped(samples);
        self.update(|s| {
            s.levels
                .observe(rms, clipped as f32 / samples.len() as f32, s.speaking);
            s.clipped_samples += clipped as u64;
            s.capture_last_ms = Some(monotonic_ms());
            s.captured_samples += samples.len() as u64;
            s.rms = rms;
            s.peak = peak;
        });
    }

    pub fn capture_stopped(&self, reason: &str) {
        self.update(|s| s.capture_error = Some(reason.chars().take(256).collect()));
    }

    pub fn frame(
        &self,
        probability: f32,
        threshold: f32,
        candidate_frames: usize,
        speaking: bool,
        rejected_peak: Option<f32>,
    ) {
        let rejected = rejected_peak.is_some();
        self.update(|s| {
            if let Some(peak) = rejected_peak {
                s.last_rejected_frames = s.candidate_frames;
                s.last_rejection_probability = probability;
                s.last_rejection_peak = peak;
            }
            s.detector_last_ms = Some(monotonic_ms());
            s.processed_frames += 1;
            s.probability = probability;
            s.threshold = threshold;
            s.candidate_frames = candidate_frames;
            s.speaking = speaking;
            s.rejected_candidates += u64::from(rejected);
        });
    }

    pub fn gain(&self, gain: f32) {
        self.update(|s| s.gain = gain);
    }

    /// A finished segment's mean probability, and whether it was dropped.
    pub fn segment(&self, mean_probability: f32, dropped: bool) {
        self.update(|s| {
            s.last_segment_mean_probability = mean_probability;
            s.dropped_segments += u64::from(dropped);
        });
    }

    pub fn set_typing_paused(&self, paused: bool) {
        self.update(|s| s.typing_paused = paused);
    }

    /// What the live stream connection is doing now.
    pub fn asr_stream(&self, status: StreamStatus) {
        self.update(|s| {
            let t = &mut s.asr_track;
            if matches!(status, StreamStatus::Up | StreamStatus::Down) {
                t.stream_settled = true;
            }
            if status == StreamStatus::Unused {
                t.stream_settled = false;
            }
            t.stream = status;
        });
    }

    pub fn asr_stream_status(&self) -> StreamStatus {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .asr_track
            .stream
    }

    /// The server answered (a transcription, a stream, a health probe).
    pub fn asr_ok(&self, path: AsrPath) {
        self.update(|s| {
            let t = &mut s.asr_track;
            t.last_ok_ms = Some(monotonic_ms());
            t.last_ok_path = Some(path);
            t.failing = false;
            t.last_error = None;
        });
    }

    /// Contact with the server failed (connection, timeout, server error).
    pub fn asr_failed(&self, error: &str) {
        self.update(|s| {
            let t = &mut s.asr_track;
            t.failing = true;
            t.last_error = Some(error.chars().take(256).collect());
        });
    }

    /// Probe the server every `every` while the live stream is not up (it
    /// proves reachability itself), and at once when the stream changes
    /// state, so `asr` stays current while nobody speaks.
    pub fn spawn_asr_probe(
        &self,
        server_url: String,
        api_key: Option<String>,
        every: Duration,
    ) -> AsrProbe {
        let health = self.clone();
        AsrProbe(tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(ASR_PROBE_TIMEOUT)
                .connect_timeout(ASR_PROBE_TIMEOUT)
                .build()
                .unwrap_or_default();
            let mut last_probe: Option<tokio::time::Instant> = None;
            let mut last_status = health.asr_stream_status();
            loop {
                let status = health.asr_stream_status();
                let changed = status != last_status;
                last_status = status;
                let due = last_probe.is_none_or(|t| t.elapsed() >= every);
                if status != StreamStatus::Up && (due || changed) {
                    last_probe = Some(tokio::time::Instant::now());
                    match probe_asr(&client, &server_url, api_key.as_deref()).await {
                        Ok(()) => health.asr_ok(AsrPath::Http),
                        Err(e) => health.asr_failed(&format!("health probe {e}")),
                    }
                }
                tokio::time::sleep(every.min(Duration::from_secs(1))).await;
            }
        }))
    }

    pub fn queue(&self, chunks: usize) {
        self.update(|s| s.queue_chunks = chunks);
    }

    /// Stage and timestamp change together; no lock survives external work.
    pub fn enter(&self, stage: Stage) -> StageGuard {
        let mut previous = Stage::Listening;
        self.update(|s| {
            previous = s.stage;
            s.stage = stage;
            s.stage_since_ms = monotonic_ms();
        });
        StageGuard {
            health: self.clone(),
            previous,
        }
    }

    pub fn snapshot(&self) -> HealthSnapshot {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        state.updated_monotonic_ms = monotonic_ms();
        state.audio_backlog_ms = state
            .captured_samples
            .saturating_sub(state.processed_frames * 512)
            / 16;
        state.noise_dbfs = state.levels.noise_dbfs();
        state.speech_dbfs = state.levels.speech_dbfs();
        state.mic_warning = state.levels.warning();
        state.asr = state.asr_track.report();
        state
    }
}

pub struct StageGuard {
    health: PipelineHealth,
    previous: Stage,
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        self.health.update(|s| {
            s.stage = self.previous;
            s.stage_since_ms = monotonic_ms();
        });
    }
}

/// Own this guard inside the pipeline task. Drop records stop even on unwind.
/// The supervisor uses its own OS thread so blocked Tokio workers cannot hide
/// a stalled capture or subprocess behind a fresh healthy status.
pub struct HealthMonitor {
    health: PipelineHealth,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
    _owner_lock: crate::lock::FileLock,
}

fn process_start_ticks(stat: &str) -> io::Result<String> {
    // comm can contain spaces and parentheses. Fields after the final ')' begin
    // with field 3; starttime is field 22 (index 19 in this remainder).
    stat.rsplit_once(')')
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .filter(|v| v.parse::<u64>().is_ok())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid /proc stat"))
}

fn publish(path: &Path, snapshot: &HealthSnapshot) -> io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", snapshot.session_id));
    let bytes = serde_json::to_vec(snapshot)?;
    fs::write(&temp, bytes)?;
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(temp);
        return Err(e);
    }
    Ok(())
}

impl HealthMonitor {
    pub fn start(state_dir: &Path) -> io::Result<Self> {
        let mut owner_lock = crate::lock::FileLock::new(state_dir.join("vad-health.lock"))
            .map_err(io::Error::other)?;
        if !owner_lock.try_lock().map_err(io::Error::other)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "desktop VAD health owner already active",
            ));
        }
        let now = monotonic_ms();
        let pid = std::process::id();
        let start_ticks = process_start_ticks(&fs::read_to_string("/proc/self/stat")?)?;
        let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_owned();
        let health = PipelineHealth(Arc::new(Mutex::new(HealthSnapshot {
            version: 1,
            pid,
            process_start_ticks: start_ticks,
            boot_id,
            session_id: format!("{pid}-{now}"),
            session_kind: "desktop_vad",
            device: String::new(),
            updated_monotonic_ms: now,
            stage: Stage::Listening,
            stage_since_ms: now,
            capture_last_ms: None,
            detector_last_ms: None,
            captured_samples: 0,
            processed_frames: 0,
            queue_chunks: 0,
            audio_backlog_ms: 0,
            rms: 0.0,
            peak: 0.0,
            probability: 0.0,
            threshold: 0.0,
            candidate_frames: 0,
            rejected_candidates: 0,
            last_rejected_frames: 0,
            last_rejection_probability: 0.0,
            last_rejection_peak: 0.0,
            gain: 1.0,
            last_segment_mean_probability: 0.0,
            dropped_segments: 0,
            speaking: false,
            capture_error: None,
            typing_paused: false,
            noise_dbfs: crate::pcm::dbfs(0.0),
            speech_dbfs: crate::pcm::dbfs(0.0),
            clipped_samples: 0,
            mic_warning: None,
            asr: AsrTrack::default().report(),
            levels: MicLevels::default(),
            asr_track: AsrTrack::default(),
        })));
        let path: PathBuf = state_dir.join(HEALTH_FILE);
        publish(&path, &health.snapshot())?;
        let (stop, rx) = mpsc::channel();
        let watched = health.clone();
        let worker = thread::Builder::new().name("ears-health".into()).spawn(move || {
            let mut previous_problem = None;
            let mut previous_mic = None;
            let mut write_failed = false;
            loop {
                let done = rx.recv_timeout(Duration::from_secs(1)) != Err(mpsc::RecvTimeoutError::Timeout);
                if done {
                    watched.update(|s| { s.stage = Stage::Stopped; s.stage_since_ms = monotonic_ms(); });
                }
                let snapshot = watched.snapshot();
                let problem = snapshot.problem(snapshot.updated_monotonic_ms);
                if done {
                    tracing::info!(session = %snapshot.session_id, "VAD health monitor stopped");
                } else if problem != previous_problem {
                    if let Some(reason) = problem {
                        tracing::warn!(session = %snapshot.session_id, stage = ?snapshot.stage, reason, "VAD health changed");
                    } else if previous_problem.is_some() {
                        tracing::info!(session = %snapshot.session_id, "VAD health recovered");
                    }
                    previous_problem = problem;
                }
                if snapshot.mic_warning != previous_mic && !done {
                    match snapshot.mic_warning {
                        Some(reason) => tracing::warn!(
                            session = %snapshot.session_id,
                            reason,
                            noise_dbfs = snapshot.noise_dbfs,
                            speech_dbfs = snapshot.speech_dbfs,
                            "Microphone quality (run ears mic-check)"
                        ),
                        None => tracing::info!(session = %snapshot.session_id, "Microphone quality recovered"),
                    }
                    previous_mic = snapshot.mic_warning;
                }
                tracing::debug!(target: "ears::health", snapshot = ?snapshot, "VAD health");
                if let Err(e) = publish(&path, &snapshot) {
                    if !write_failed { tracing::warn!("Cannot publish VAD health: {e}"); }
                    write_failed = true;
                } else { write_failed = false; }
                if done { break; }
            }
        })?;
        Ok(Self {
            health,
            stop,
            worker: Some(worker),
            _owner_lock: owner_lock,
        })
    }

    pub fn health(&self) -> PipelineHealth {
        self.health.clone()
    }
}

impl Drop for HealthMonitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_guards_restore_on_error_and_snapshot_is_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        {
            let _detect = health.enter(Stage::Detecting);
            let _transcribe = health.enter(Stage::Transcribing);
            assert_eq!(health.snapshot().stage, Stage::Transcribing);
        }
        assert_eq!(health.snapshot().stage, Stage::Listening);
        drop(monitor);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(HEALTH_FILE)).unwrap()).unwrap();
        assert_eq!(value["stage"], "stopped");
        assert_eq!(value["session_kind"], "desktop_vad");
    }

    #[test]
    fn silence_is_live_but_capture_failure_and_slow_work_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        health.captured(&[0.0; 1600]);
        let snap = health.snapshot();
        assert_eq!(snap.rms, 0.0);
        assert_eq!(snap.problem(snap.updated_monotonic_ms), None);
        assert_eq!(
            snap.problem(snap.updated_monotonic_ms + 4000),
            Some("no audio arriving")
        );
        let _stage = health.enter(Stage::Typing);
        let mut snap = health.snapshot();
        snap.capture_last_ms = Some(snap.stage_since_ms + 6000);
        assert_eq!(
            snap.problem(snap.stage_since_ms + 6000),
            Some("slow pipeline stage")
        );
        health.capture_stopped("EOF");
        let snap = health.snapshot();
        assert_eq!(
            snap.problem(snap.updated_monotonic_ms),
            Some("capture stopped")
        );
    }

    #[test]
    fn mic_warnings_follow_the_live_levels() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        let chunk = |level: f32| -> Vec<f32> {
            (0..1600)
                .map(|i| if i % 2 == 0 { level } else { -level })
                .collect()
        };
        // Too little audio to judge yet.
        health.captured(&[0.0; 1600]);
        assert_eq!(health.snapshot().mic_warning, None);
        for _ in 0..MIC_SETTLE_CHUNKS {
            health.captured(&[0.0; 1600]);
        }
        assert_eq!(health.snapshot().mic_warning, Some("mic silent"));

        // A quiet room and normal speech.
        for _ in 0..200 {
            health.captured(&chunk(0.001));
        }
        health.frame(0.9, 0.5, 5, true, None);
        for _ in 0..MIC_SETTLE_CHUNKS {
            health.captured(&chunk(0.1));
        }
        let snap = health.snapshot();
        assert_eq!(snap.mic_warning, None, "{snap:?}");
        assert!((snap.speech_dbfs + 20.0).abs() < 0.5);
        assert!((snap.noise_dbfs + 60.0).abs() < 0.5);

        // Quiet speech.
        for _ in 0..60 {
            health.captured(&chunk(0.003));
        }
        assert_eq!(health.snapshot().mic_warning, Some("speech too quiet"));

        // Clipping wins, and is counted.
        for _ in 0..60 {
            health.captured(&chunk(1.0));
        }
        let snap = health.snapshot();
        assert_eq!(snap.mic_warning, Some("mic clipping"));
        assert!(snap.clipped_samples >= 60 * 1600);
        let value: serde_json::Value = serde_json::to_value(&snap).unwrap();
        assert_eq!(value["mic_warning"], "mic clipping");
        assert!(value.get("levels").is_none());
    }

    #[test]
    fn probability_observations_count_rejections_without_retaining_audio() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        health.frame(0.8, 0.5, 1, false, None);
        health.frame(0.2, 0.5, 0, false, Some(0.8));
        let snap = health.snapshot();
        assert_eq!(snap.processed_frames, 2);
        assert_eq!(snap.rejected_candidates, 1);
        assert_eq!(snap.last_rejected_frames, 1);
        assert_eq!(snap.last_rejection_probability, 0.2);
        assert_eq!(snap.last_rejection_peak, 0.8);
        assert_eq!(snap.probability, 0.2);
    }

    #[test]
    fn backlog_is_observable_while_consumer_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        for _ in 0..25 {
            health.captured(&[0.0; 1600]);
        }
        let snap = health.snapshot();
        assert_eq!(snap.audio_backlog_ms, 2500);
        assert_eq!(
            snap.problem(snap.updated_monotonic_ms),
            Some("audio backlog")
        );
        for _ in 0..78 {
            health.frame(0.0, 0.5, 0, false, None);
        }
        assert_eq!(health.snapshot().audio_backlog_ms, 4);
    }

    #[test]
    fn ordinary_transcription_latency_is_not_a_stall() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        let _stage = health.enter(Stage::Transcribing);
        let mut snap = health.snapshot();
        let now = snap.stage_since_ms + 9000;
        snap.capture_last_ms = Some(now);
        snap.audio_backlog_ms = 9000;
        assert_eq!(snap.problem(now), None);
        snap.capture_last_ms = Some(now + 15000);
        assert_eq!(snap.problem(now + 15000), Some("slow pipeline stage"));
    }

    #[test]
    fn observer_recovers_after_poisoned_update() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        let _ = std::panic::catch_unwind(|| health.update(|_| panic!("injected observer panic")));
        health.captured(&[0.0; 512]);
        assert_eq!(health.snapshot().captured_samples, 512);
    }

    #[test]
    fn uncertain_output_remains_visible_while_capture_is_alive() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        health.captured(&[0.0; 512]);
        health.set_typing_paused(true);
        let snap = health.snapshot();
        assert_eq!(
            snap.problem(snap.updated_monotonic_ms),
            Some("typing paused after output error")
        );
        health.set_typing_paused(false);
        let snap = health.snapshot();
        assert_eq!(snap.problem(snap.updated_monotonic_ms), None);
    }

    #[test]
    fn competing_monitor_cannot_overwrite_owner() {
        let dir = tempfile::tempdir().unwrap();
        let first = HealthMonitor::start(dir.path()).unwrap();
        assert!(HealthMonitor::start(dir.path()).is_err());
        drop(first);
        assert!(HealthMonitor::start(dir.path()).is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_reports_stall_even_with_blocked_async_executor() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        health.captured(&[0.0; 512]);
        let _stage = health.enter(Stage::Typing);
        health.update(|s| s.stage_since_ms = monotonic_ms().saturating_sub(6000));
        // Deliberately block the only Tokio worker, as a stuck typing child did.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(dir.path().join(HEALTH_FILE)).unwrap()).unwrap();
            if value["stage"] == "typing" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "supervisor did not publish"
            );
            thread::sleep(Duration::from_millis(20));
        }
        // Keep this assertion about stage progress, even if the CI scheduler
        // delayed the supervisor enough for the earlier audio sample to age.
        health.captured(&[0.0; 512]);
        let snap = health.snapshot();
        assert_eq!(
            snap.problem(snap.updated_monotonic_ms),
            Some("slow pipeline stage")
        );
    }

    #[test]
    fn parses_process_identity_with_spaces_and_parentheses() {
        let stat = format!("123 (ears (test)) S {} 9876 0", vec!["0"; 18].join(" "));
        assert_eq!(process_start_ticks(&stat).unwrap(), "9876");
        assert!(process_start_ticks("bad").is_err());
    }

    #[test]
    fn asr_state_follows_stream_and_requests() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        assert_eq!(health.snapshot().asr.state, AsrState::Unknown);
        health.asr_ok(AsrPath::Http);
        let asr = health.snapshot().asr;
        assert_eq!((asr.state, asr.path), (AsrState::Ok, Some(AsrPath::Http)));
        assert!(asr.last_ok_ms.is_some());
        // A first connect is not a degradation; a reconnect is.
        health.asr_stream(StreamStatus::Connecting);
        assert_eq!(health.snapshot().asr.state, AsrState::Ok);
        health.asr_stream(StreamStatus::Up);
        let asr = health.snapshot().asr;
        assert_eq!((asr.state, asr.path), (AsrState::Ok, Some(AsrPath::Stream)));
        health.asr_stream(StreamStatus::Connecting);
        assert_eq!(health.snapshot().asr.state, AsrState::Degraded);
        health.asr_stream(StreamStatus::Down);
        assert_eq!(health.snapshot().asr.state, AsrState::Degraded);
        health.asr_failed("connection refused");
        let snap = health.snapshot();
        assert_eq!((snap.asr.state, snap.asr.path), (AsrState::Down, None));
        assert_eq!(snap.asr.last_error.as_deref(), Some("connection refused"));
        assert_eq!(
            snap.problem(snap.updated_monotonic_ms),
            Some("transcription server unreachable")
        );
        health.asr_ok(AsrPath::Http);
        assert_eq!(health.snapshot().asr.state, AsrState::Degraded);
        drop(monitor);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(HEALTH_FILE)).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["asr"]["state"], "degraded");
        assert_eq!(value["asr"]["path"], "http");
        assert!(value["asr"]["last_ok_ms"].is_u64());
        assert!(value["asr"]["last_error"].is_null());
    }

    #[tokio::test]
    async fn asr_probe_marks_the_server_up_and_down() {
        use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        let server = MockServer::start().await;
        Mock::given(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let probe = health.spawn_asr_probe(server.uri(), None, Duration::from_millis(20));
        for _ in 0..200 {
            if health.snapshot().asr.state == AsrState::Ok {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(health.snapshot().asr.state, AsrState::Ok);
        drop(probe);

        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let _probe = health.spawn_asr_probe(
            format!("http://127.0.0.1:{port}"),
            None,
            Duration::from_millis(20),
        );
        for _ in 0..200 {
            if health.snapshot().asr.state == AsrState::Down {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let asr = health.snapshot().asr;
        assert_eq!(asr.state, AsrState::Down);
        assert!(asr.last_error.unwrap().starts_with("health probe /health"));
    }
}
