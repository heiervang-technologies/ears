//! Volume ducking for VAD mode
//!
//! Lowers the system audio sink volume when speech is detected and restores
//! it when speech ends. Best-effort: failures (missing `wpctl`, parse errors)
//! are logged and ignored.
//!
//! Uses `wpctl @DEFAULT_AUDIO_SINK@` (PipeWire/WirePlumber) which is the
//! standard on Omarchy/Arch + PipeWire systems.
//!
//! ## Ordering guarantees
//!
//! Duck and restore both run against the sink asynchronously. Two things keep
//! them from fighting:
//!
//! - A per-ducker **epoch** is bumped by every restore/cancel. A duck that
//!   was requested before the bump aborts before it touches the sink, so a
//!   late `wpctl get-volume` cannot resurrect a duck that was already undone.
//! - All sink operations are serialized through one async lock, so a restore
//!   requested while a duck is mid-flight waits for the duck to finish and
//!   then undoes it, rather than racing it.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Something that can read and write the output volume.
///
/// The real implementation shells out to `wpctl`. Tests substitute a fake to
/// assert the duck/restore ordering without a sound server.
pub trait VolumeBackend: Send + Sync + 'static {
    /// Current sink volume (0.0-1.0, may exceed 1.0 with boost). `None` on failure.
    fn get_volume(&self) -> Option<f32>;
    /// Set the sink volume. Best-effort; failures are logged by the backend.
    fn set_volume(&self, volume: f32);
}

/// `wpctl @DEFAULT_AUDIO_SINK@` backend.
pub struct WpctlBackend;

/// Deadline for one `wpctl` call. WirePlumber answers in milliseconds; a
/// wedged session bus must not pin a ducker task (or runtime shutdown) forever.
const WPCTL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

impl VolumeBackend for WpctlBackend {
    fn get_volume(&self) -> Option<f32> {
        let mut cmd = Command::new("wpctl");
        cmd.args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let output = match crate::desktop::output_bounded(cmd, WPCTL_TIMEOUT) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("wpctl get-volume failed: {}", e);
                return None;
            }
        };
        if !output.status.success() {
            tracing::warn!("wpctl get-volume failed with status {}", output.status);
            return None;
        }
        parse_volume(&String::from_utf8_lossy(&output.stdout))
    }

    fn set_volume(&self, volume: f32) {
        let v = volume.clamp(0.0, 1.5); // Cap at 150% to avoid runaway boost.
        let arg = format!("{:.4}", v);
        let mut cmd = Command::new("wpctl");
        cmd.args(["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Err(e) = crate::desktop::run_bounded(cmd, WPCTL_TIMEOUT) {
            tracing::warn!("wpctl set-volume failed: {}", e);
        }
    }
}

/// Manages volume ducking lifecycle.
///
/// Cheap to clone: settings and saved-volume state are shared via Arc.
/// Drop impl restores the saved volume on a best-effort basis.
#[derive(Clone)]
pub struct VolumeDucker {
    inner: Arc<Inner>,
}

struct Inner {
    /// Settings: ducking enabled + reduction percent (0-100)
    settings: Mutex<DuckSettings>,
    /// Pre-duck volume (0.0-1.0). Some => currently ducked, None => not ducked.
    saved_volume: Mutex<Option<f32>>,
    /// Bumped on every restore/cancel; in-flight ducks from an older epoch abort.
    epoch: AtomicU64,
    /// Serializes sink operations so duck and restore never interleave.
    op_lock: tokio::sync::Mutex<()>,
    /// Volume backend (wpctl in production, fake in tests).
    backend: Arc<dyn VolumeBackend>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DuckSettings {
    enabled: bool,
    percent: u8,
}

impl VolumeDucker {
    pub fn new(enabled: bool, percent: u8) -> Self {
        Self::with_backend(enabled, percent, Arc::new(WpctlBackend))
    }

    /// Construct with an explicit backend (used by tests).
    pub fn with_backend(enabled: bool, percent: u8, backend: Arc<dyn VolumeBackend>) -> Self {
        Self {
            inner: Arc::new(Inner {
                settings: Mutex::new(DuckSettings {
                    enabled,
                    percent: percent.min(100),
                }),
                saved_volume: Mutex::new(None),
                epoch: AtomicU64::new(0),
                op_lock: tokio::sync::Mutex::new(()),
                backend,
            }),
        }
    }

    /// Update settings live. Does not affect any in-flight duck.
    pub fn set_settings(&self, enabled: bool, percent: u8) {
        if let Ok(mut s) = self.inner.settings.lock() {
            s.enabled = enabled;
            s.percent = percent.min(100);
        }
    }

    /// Whether the sink is currently ducked (a pre-duck volume is saved).
    pub fn is_ducked(&self) -> bool {
        matches!(self.inner.saved_volume.lock(), Ok(g) if g.is_some())
    }

    /// Called when VAD signals probable speech. Saves current volume and
    /// reduces it. No-op if disabled, percent is 0, or already ducked.
    pub fn on_speech_probable(&self) {
        let settings = match self.inner.settings.lock() {
            Ok(s) => *s,
            Err(_) => return,
        };
        if !settings.enabled || settings.percent == 0 {
            return;
        }

        // Skip if already ducked (e.g. SpeechProbable fired twice without End)
        if self.is_ducked() {
            return;
        }

        let inner = self.inner.clone();
        let percent = settings.percent;
        let requested_epoch = inner.epoch.load(Ordering::SeqCst);
        tokio::spawn(async move {
            let _guard = inner.op_lock.lock().await;
            // A restore/cancel landed before we got the lock: this duck is stale.
            if inner.epoch.load(Ordering::SeqCst) != requested_epoch {
                tracing::debug!("Duck request superseded before it ran; skipping");
                return;
            }
            let backend = inner.backend.clone();
            let current = match tokio::task::spawn_blocking(move || backend.get_volume()).await {
                Ok(Some(v)) => v,
                _ => return,
            };
            // Re-check: get_volume ran external work, a cancel may have raced it.
            if inner.epoch.load(Ordering::SeqCst) != requested_epoch {
                tracing::debug!("Duck request superseded during volume read; skipping");
                return;
            }
            // Persist saved volume first so on_speech_ended can restore.
            if let Ok(mut g) = inner.saved_volume.lock() {
                if g.is_some() {
                    return; // Raced — someone else saved already.
                }
                *g = Some(current);
            }
            let factor = 1.0 - (percent as f32 / 100.0);
            let target = (current * factor).clamp(0.0, 1.0);
            let backend = inner.backend.clone();
            let _ = tokio::task::spawn_blocking(move || backend.set_volume(target)).await;
            tracing::debug!(
                "Ducked volume: {:.2} -> {:.2} (-{}%)",
                current,
                target,
                percent
            );
        });
    }

    /// Called when VAD signals end of speech. Restores saved volume.
    pub fn on_speech_ended(&self) {
        self.restore("speech ended");
    }

    /// Called when a probable-speech candidate was rejected before it was
    /// confirmed. There is no completed utterance, but the sink was ducked on
    /// the probable event, so it must be restored the same way.
    pub fn on_speech_rejected(&self) {
        self.restore("candidate rejected");
    }

    /// Invalidate any pending duck and restore the sink if it was ducked.
    fn restore(&self, why: &'static str) {
        // Bump first: any duck still waiting on the lock sees a new epoch and aborts.
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
        let inner = self.inner.clone();

        // Outside a runtime (shutdown paths, sync tests) there can be no
        // in-flight duck task, so a plain synchronous restore is exact.
        if tokio::runtime::Handle::try_current().is_err() {
            let saved = match inner.saved_volume.lock() {
                Ok(mut g) => g.take(),
                Err(_) => return,
            };
            if let Some(volume) = saved {
                inner.backend.set_volume(volume);
                tracing::debug!("Restored volume: {:.2} ({}, sync)", volume, why);
            }
            return;
        }

        tokio::spawn(async move {
            let _guard = inner.op_lock.lock().await;
            // Take *inside* the lock so a duck that just finished is observed.
            let saved = match inner.saved_volume.lock() {
                Ok(mut g) => g.take(),
                Err(_) => return,
            };
            let Some(volume) = saved else { return };
            let backend = inner.backend.clone();
            let _ = tokio::task::spawn_blocking(move || backend.set_volume(volume)).await;
            tracing::debug!("Restored volume: {:.2} ({})", volume, why);
        });
    }

    /// Wait until no sink operation is in flight. Test/shutdown helper.
    pub async fn quiesce(&self) {
        let _guard = self.inner.op_lock.lock().await;
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Best-effort sync restore on drop. Cannot use tokio here since the
        // runtime may already be gone (e.g. Ctrl-C path).
        let saved = match self.saved_volume.lock() {
            Ok(mut g) => g.take(),
            Err(_) => return,
        };
        if let Some(volume) = saved {
            self.backend.set_volume(volume);
        }
    }
}

/// Parse a `wpctl get-volume` output line.
///
/// Examples:
/// - "Volume: 0.42" -> Some(0.42)
/// - "Volume: 0.42 [MUTED]" -> Some(0.42)
/// - "" -> None
fn parse_volume(stdout: &str) -> Option<f32> {
    let line = stdout.lines().next()?;
    let after = line.split_once("Volume:").map(|(_, rest)| rest.trim())?;
    let token = after.split_whitespace().next()?;
    token.parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Fake sink: records every set, optionally delays get so races can be staged.
    struct FakeBackend {
        volume: Mutex<f32>,
        sets: Mutex<Vec<f32>>,
        get_delay: Duration,
    }

    impl FakeBackend {
        fn new(volume: f32, get_delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                volume: Mutex::new(volume),
                sets: Mutex::new(Vec::new()),
                get_delay,
            })
        }
        fn sets(&self) -> Vec<f32> {
            self.sets.lock().unwrap().clone()
        }
        fn volume(&self) -> f32 {
            *self.volume.lock().unwrap()
        }
    }

    impl VolumeBackend for FakeBackend {
        fn get_volume(&self) -> Option<f32> {
            std::thread::sleep(self.get_delay);
            Some(*self.volume.lock().unwrap())
        }
        fn set_volume(&self, v: f32) {
            *self.volume.lock().unwrap() = v;
            self.sets.lock().unwrap().push(v);
        }
    }

    async fn settle(d: &VolumeDucker) {
        // Let spawned tasks get scheduled, then wait for the op lock to be free.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        d.quiesce().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        d.quiesce().await;
    }

    #[test]
    fn parse_basic() {
        assert_eq!(parse_volume("Volume: 0.42\n"), Some(0.42));
    }

    #[test]
    fn parse_muted() {
        assert_eq!(parse_volume("Volume: 0.65 [MUTED]\n"), Some(0.65));
    }

    #[test]
    fn parse_no_newline() {
        assert_eq!(parse_volume("Volume: 1.00"), Some(1.00));
    }

    #[test]
    fn parse_invalid() {
        assert_eq!(parse_volume(""), None);
        assert_eq!(parse_volume("nope"), None);
        assert_eq!(parse_volume("Volume: not_a_number"), None);
    }

    #[test]
    fn settings_clamped() {
        let d = VolumeDucker::new(true, 200);
        let s = d.inner.settings.lock().unwrap();
        assert_eq!(s.percent, 100);
    }

    #[test]
    fn set_settings_clamps() {
        let d = VolumeDucker::new(false, 0);
        d.set_settings(true, 250);
        let s = d.inner.settings.lock().unwrap();
        assert!(s.enabled);
        assert_eq!(s.percent, 100);
    }

    #[test]
    fn disabled_no_op() {
        // Just verify it doesn't panic when disabled — doesn't call wpctl.
        let d = VolumeDucker::new(false, 50);
        d.on_speech_probable();
        assert!(!d.is_ducked());
    }

    #[tokio::test]
    async fn duck_then_restore() {
        let fake = FakeBackend::new(0.8, Duration::ZERO);
        let d = VolumeDucker::with_backend(true, 50, fake.clone());

        d.on_speech_probable();
        settle(&d).await;
        assert!(d.is_ducked());
        assert!((fake.volume() - 0.4).abs() < 1e-4);

        d.on_speech_ended();
        settle(&d).await;
        assert!(!d.is_ducked());
        assert!((fake.volume() - 0.8).abs() < 1e-4);
        assert_eq!(fake.sets().len(), 2);
    }

    #[tokio::test]
    async fn rejected_candidate_restores() {
        let fake = FakeBackend::new(0.8, Duration::ZERO);
        let d = VolumeDucker::with_backend(true, 50, fake.clone());

        d.on_speech_probable();
        settle(&d).await;
        assert!(d.is_ducked());

        d.on_speech_rejected();
        settle(&d).await;
        assert!(!d.is_ducked(), "rejected candidate must restore volume");
        assert!((fake.volume() - 0.8).abs() < 1e-4);
    }

    #[tokio::test]
    async fn late_duck_after_restore_is_dropped() {
        // get_volume is slow; restore lands while the duck is mid-flight.
        let fake = FakeBackend::new(0.8, Duration::from_millis(60));
        let d = VolumeDucker::with_backend(true, 50, fake.clone());

        d.on_speech_probable();
        tokio::time::sleep(Duration::from_millis(15)).await; // duck is inside get_volume
        d.on_speech_rejected();
        settle(&d).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        d.quiesce().await;

        assert!(!d.is_ducked(), "late duck must not leave the sink ducked");
        assert!(
            (fake.volume() - 0.8).abs() < 1e-4,
            "volume must be untouched"
        );
        assert!(fake.sets().is_empty(), "no set should have happened");
    }

    #[tokio::test]
    async fn restore_before_duck_ran_cancels_it() {
        let fake = FakeBackend::new(0.8, Duration::ZERO);
        let d = VolumeDucker::with_backend(true, 50, fake.clone());

        // Both requests queued before either task runs.
        d.on_speech_probable();
        d.on_speech_ended();
        settle(&d).await;

        assert!(!d.is_ducked());
        assert!(fake.sets().is_empty());
    }

    #[tokio::test]
    async fn repeated_probable_ducks_once() {
        let fake = FakeBackend::new(1.0, Duration::ZERO);
        let d = VolumeDucker::with_backend(true, 25, fake.clone());

        d.on_speech_probable();
        settle(&d).await;
        d.on_speech_probable();
        settle(&d).await;
        assert_eq!(fake.sets().len(), 1);
        assert!((fake.volume() - 0.75).abs() < 1e-4);

        d.on_speech_ended();
        settle(&d).await;
        assert!((fake.volume() - 1.0).abs() < 1e-4);
    }
}
