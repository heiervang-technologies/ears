//! Volume ducking for VAD mode
//!
//! Lowers the system audio sink volume when speech is detected and restores
//! it when speech ends. Best-effort: failures (missing `wpctl`, parse errors)
//! are logged and ignored.
//!
//! Uses `wpctl @DEFAULT_AUDIO_SINK@` (PipeWire/WirePlumber) which is the
//! standard on Omarchy/Arch + PipeWire systems.

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::process::Command as TokioCommand;

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
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DuckSettings {
    enabled: bool,
    percent: u8,
}

impl VolumeDucker {
    pub fn new(enabled: bool, percent: u8) -> Self {
        Self {
            inner: Arc::new(Inner {
                settings: Mutex::new(DuckSettings {
                    enabled,
                    percent: percent.min(100),
                }),
                saved_volume: Mutex::new(None),
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
        if matches!(self.inner.saved_volume.lock(), Ok(g) if g.is_some()) {
            return;
        }

        let inner = self.inner.clone();
        let percent = settings.percent;
        tokio::spawn(async move {
            let current = match get_volume_async().await {
                Some(v) => v,
                None => return,
            };
            // Persist saved volume first so on_speech_ended can restore.
            if let Ok(mut g) = inner.saved_volume.lock() {
                if g.is_some() {
                    return; // Raced — someone else saved already.
                }
                *g = Some(current);
            }
            let factor = 1.0 - (percent as f32 / 100.0);
            let target = (current * factor).clamp(0.0, 1.0);
            set_volume_async(target).await;
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
        let saved = match self.inner.saved_volume.lock() {
            Ok(mut g) => g.take(),
            Err(_) => return,
        };
        let Some(volume) = saved else { return };
        tokio::spawn(async move {
            set_volume_async(volume).await;
            tracing::debug!("Restored volume: {:.2}", volume);
        });
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
            let _ = Command::new("wpctl")
                .args([
                    "set-volume",
                    "@DEFAULT_AUDIO_SINK@",
                    &format!("{:.4}", volume),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null())
                .status();
        }
    }
}

/// Run `wpctl get-volume @DEFAULT_AUDIO_SINK@` and parse the volume.
///
/// Output format: "Volume: 0.42" or "Volume: 0.42 [MUTED]". Returns the
/// raw value (typically 0.0-1.0, but PipeWire allows boost above 1.0).
async fn get_volume_async() -> Option<f32> {
    let output = TokioCommand::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        tracing::warn!(
            "wpctl get-volume failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    parse_volume(&String::from_utf8_lossy(&output.stdout))
}

async fn set_volume_async(volume: f32) {
    let v = volume.clamp(0.0, 1.5); // Cap at 150% to avoid runaway boost.
    let arg = format!("{:.4}", v);
    let result = TokioCommand::new("wpctl")
        .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    if let Err(e) = result {
        tracing::warn!("wpctl set-volume failed: {}", e);
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
        d.on_speech_ended();
    }
}
