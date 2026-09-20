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
    pub speaking: bool,
    pub capture_error: Option<String>,
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
        if self.stage != Stage::Listening && now.saturating_sub(self.stage_since_ms) > 5000 {
            return Some("slow pipeline stage");
        }
        if self.stage == Stage::Listening
            && now.saturating_sub(self.detector_last_ms.unwrap_or(self.stage_since_ms)) > 3000
        {
            return Some("voice detection stalled");
        }
        if self.audio_backlog_ms > 2000 {
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
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        let rms = (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64)
            .sqrt() as f32;
        self.update(|s| {
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
        rejected: bool,
    ) {
        self.update(|s| {
            s.detector_last_ms = Some(monotonic_ms());
            s.processed_frames += 1;
            s.probability = probability;
            s.threshold = threshold;
            s.candidate_frames = candidate_frames;
            s.speaking = speaking;
            s.rejected_candidates += u64::from(rejected);
        });
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
            speaking: false,
            capture_error: None,
        })));
        let path: PathBuf = state_dir.join(HEALTH_FILE);
        publish(&path, &health.snapshot())?;
        let (stop, rx) = mpsc::channel();
        let watched = health.clone();
        let worker = thread::Builder::new().name("ears-health".into()).spawn(move || {
            let mut previous_problem = None;
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
    fn probability_observations_count_rejections_without_retaining_audio() {
        let dir = tempfile::tempdir().unwrap();
        let monitor = HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        health.frame(0.8, 0.5, 1, false, false);
        health.frame(0.2, 0.5, 0, false, true);
        let snap = health.snapshot();
        assert_eq!(snap.processed_frames, 2);
        assert_eq!(snap.rejected_candidates, 1);
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
            health.frame(0.0, 0.5, 0, false, false);
        }
        assert_eq!(health.snapshot().audio_backlog_ms, 4);
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
}
