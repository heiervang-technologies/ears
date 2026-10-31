//! Which ASR server ears uses: the configured `server`, else the first
//! healthy entry of `server_fallback`, in order.
//!
//! A transcription that cannot reach the active server (connect error or
//! timeout) moves to the next entry at once. The health probe re-checks the
//! entries above the active one and moves back up as soon as one answers,
//! so the primary wins again when it returns. A `batch_only` entry gets
//! final transcriptions only: no live stream and no previews (a small CPU
//! decoding every 300 ms would fall behind).
//!
//! `vad-health.json` reports the choice at the top level: `asr_backend`
//! (`primary`, `fallback:N` counting from 1, or `none`), `asr_url` and
//! `asr_since` (Unix seconds of the last switch).

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// One server in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrEntry {
    pub url: String,
    /// Finals only: no live stream, no previews.
    pub batch_only: bool,
}

/// The server list and which entry is in use. Entry 0 is the primary.
#[derive(Debug)]
pub struct AsrRoute {
    entries: Vec<AsrEntry>,
    state: Mutex<State>,
}

#[derive(Debug, Clone, Copy)]
struct State {
    /// None: every entry failed; the probe looks for one that answers.
    active: Option<usize>,
    since: f64,
}

/// What `vad-health.json` shows.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RouteStatus {
    pub asr_backend: String,
    pub asr_url: Option<String>,
    pub asr_since: f64,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl AsrRoute {
    /// The primary, then the fallbacks in order. Starts on the primary.
    pub fn new(primary: impl Into<String>, fallbacks: Vec<AsrEntry>) -> Self {
        let mut entries = vec![AsrEntry {
            url: primary.into(),
            batch_only: false,
        }];
        entries.extend(fallbacks);
        Self {
            entries,
            state: Mutex::new(State {
                active: Some(0),
                since: now(),
            }),
        }
    }

    fn state(&self) -> State {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn entries(&self) -> &[AsrEntry] {
        &self.entries
    }

    pub fn has_fallbacks(&self) -> bool {
        self.entries.len() > 1
    }

    /// Index of the entry in use, if any.
    pub fn active(&self) -> Option<usize> {
        self.state().active
    }

    /// URL to send requests to: the active entry, or the primary while none
    /// answers (requests then fail and are reported as before).
    pub fn url(&self) -> String {
        let i = self.active().unwrap_or(0);
        self.entries[i].url.clone()
    }

    /// Whether live decoding (stream or previews) may use the active entry.
    pub fn live_ok(&self) -> bool {
        self.active().is_some_and(|i| !self.entries[i].batch_only)
    }

    /// Use entry `to` (None: nothing answers). Logs and stamps the switch.
    pub fn switch(&self, to: Option<usize>) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.active == to {
            return;
        }
        let label = |i: Option<usize>| match i {
            Some(i) => format!("{} ({})", backend(Some(i)), self.entries[i].url),
            None => "none".to_string(),
        };
        if to.is_some_and(|t| s.active.is_none_or(|a| t < a)) {
            info!("ASR: {} -> {}", label(s.active), label(to));
        } else {
            warn!("ASR: {} -> {}", label(s.active), label(to));
        }
        s.active = to;
        s.since = now();
    }

    /// The entry at `index` could not be reached: move on to the next one,
    /// if it is the one in use. Returns the entry now in use.
    pub fn failed(&self, index: usize) -> Option<usize> {
        let active = self.active();
        if active == Some(index) {
            let next = index + 1;
            self.switch((next < self.entries.len()).then_some(next));
        }
        self.active()
    }

    /// Entries worth re-probing: all above the active one (all while none).
    pub fn above_active(&self) -> std::ops::Range<usize> {
        0..self.active().unwrap_or(self.entries.len())
    }

    pub fn status(&self) -> RouteStatus {
        let s = self.state();
        RouteStatus {
            asr_backend: backend(s.active),
            asr_url: s.active.map(|i| self.entries[i].url.clone()),
            asr_since: s.since,
        }
    }
}

/// `primary`, `fallback:N` (N from 1), or `none`.
fn backend(active: Option<usize>) -> String {
    match active {
        Some(0) => "primary".into(),
        Some(i) => format!("fallback:{i}"),
        None => "none".into(),
    }
}

/// Whether a request error means the server was not reached (or did not
/// answer in time), as opposed to a server that answered badly.
pub fn unreachable(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> AsrRoute {
        AsrRoute::new(
            "http://centurion:30189/",
            vec![
                AsrEntry {
                    url: "http://192.168.78.1:30195/".into(),
                    batch_only: false,
                },
                AsrEntry {
                    url: "http://127.0.0.1:30195/".into(),
                    batch_only: true,
                },
            ],
        )
    }

    #[test]
    fn starts_on_the_primary() {
        let r = route();
        assert_eq!(r.active(), Some(0));
        assert_eq!(r.status().asr_backend, "primary");
        assert_eq!(r.status().asr_url.as_deref(), Some("http://centurion:30189/"));
        assert!(r.live_ok());
        assert_eq!(r.above_active(), 0..0);
    }

    #[test]
    fn failures_walk_down_the_list_then_to_none() {
        let r = route();
        assert_eq!(r.failed(0), Some(1));
        assert_eq!(r.status().asr_backend, "fallback:1");
        assert!(r.live_ok());
        assert_eq!(r.failed(1), Some(2));
        assert_eq!(r.url(), "http://127.0.0.1:30195/");
        assert!(!r.live_ok(), "batch-only entry: no live decoding");
        assert_eq!(r.failed(2), None);
        let s = r.status();
        assert_eq!((s.asr_backend.as_str(), s.asr_url), ("none", None));
        assert_eq!(r.url(), "http://centurion:30189/", "none: requests go to the primary");
        assert!(!r.live_ok());
        assert_eq!(r.above_active(), 0..3, "none: probe everything");
    }

    #[test]
    fn a_stale_failure_does_not_move_the_route() {
        // two requests in flight against the primary both fail: only one move
        let r = route();
        r.failed(0);
        assert_eq!(r.failed(0), Some(1));
    }

    #[test]
    fn switching_back_up_stamps_since() {
        let r = route();
        r.failed(0);
        let before = r.status().asr_since;
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(r.above_active(), 0..1);
        r.switch(Some(0));
        assert_eq!(r.status().asr_backend, "primary");
        assert!(r.status().asr_since > before);
        let unchanged = r.status().asr_since;
        r.switch(Some(0));
        assert_eq!(r.status().asr_since, unchanged, "no switch, no new stamp");
    }

    #[test]
    fn without_fallbacks_the_route_is_just_the_server() {
        let r = AsrRoute::new("http://only:1/", vec![]);
        assert!(!r.has_fallbacks());
        assert_eq!(r.failed(0), None);
        assert_eq!(r.url(), "http://only:1/");
    }
}
