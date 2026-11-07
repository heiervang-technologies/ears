//! `ears health`: read `vad-health.json` and say whether desktop VAD works.
//!
//! The snapshot is read as loose JSON: a field this build does not know is
//! passed through, a missing one is reported as unknown. Nothing here writes.
use crate::health::{monotonic_ms, process_start_ticks, Progress, Stage, HEALTH_FILE};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

/// The writer publishes once per second; older than this is not reporting.
pub const STALE_AFTER_MS: u64 = 5000;

/// The verdict, and the exit code of `ears health`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Healthy,
    /// No VAD: never started, stopped, or its process is gone.
    NotRunning,
    /// A VAD process exists but its snapshot is stale or reports a problem.
    Stuck,
    /// Working, with something worth a look (microphone, degraded ASR).
    Warning,
    /// The snapshot cannot be interpreted.
    Unreadable,
}

impl Status {
    pub fn exit_code(self) -> i32 {
        match self {
            Status::Healthy => 0,
            Status::NotRunning => 1,
            Status::Stuck => 2,
            Status::Warning => 3,
            Status::Unreadable => 4,
        }
    }
}

/// What the reader knows about this machine; tests supply their own.
pub struct Host<'a> {
    /// CLOCK_MONOTONIC now, the clock of the snapshot's `*_ms` fields.
    pub now_ms: u64,
    pub boot_id: &'a str,
    /// Start time (clock ticks) of a live pid, None when there is no such
    /// process. Told apart from a recycled pid by the snapshot's own ticks.
    pub start_ticks: &'a dyn Fn(u32) -> Option<String>,
    /// `vad.pid` names a live process (used when there is no snapshot).
    pub pid_file_alive: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub status: Status,
    pub exit_code: i32,
    /// One line: why this status.
    pub summary: String,
    /// The snapshot's process is alive and is the one that wrote it.
    pub running: bool,
    /// The snapshot was updated within [`STALE_AFTER_MS`].
    pub fresh: Option<bool>,
    pub age_ms: Option<u64>,
    pub stage_for_ms: Option<u64>,
    /// Why the pipeline is stuck (the writer's own diagnosis rules).
    pub problem: Option<String>,
    pub warnings: Vec<String>,
    /// `vad-health.json` as published, unknown fields included.
    pub snapshot: Option<Value>,
}

impl Report {
    fn new(status: Status, summary: impl Into<String>, snapshot: Option<Value>) -> Self {
        Self {
            status,
            exit_code: status.exit_code(),
            summary: summary.into(),
            running: false,
            fresh: None,
            age_ms: None,
            stage_for_ms: None,
            problem: None,
            warnings: Vec::new(),
            snapshot,
        }
    }
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn uint(v: &Value, key: &str) -> Option<u64> {
    v.get(key)?.as_u64()
}

/// Judge the published snapshot (`raw`: the file's contents, None when there
/// is no file).
pub fn evaluate(raw: Option<&str>, host: &Host) -> Report {
    let Some(raw) = raw else {
        return if host.pid_file_alive {
            Report {
                running: true,
                ..Report::new(
                    Status::Stuck,
                    "a VAD process runs but has published no health snapshot",
                    None,
                )
            }
        } else {
            Report::new(Status::NotRunning, "no VAD has run in this session", None)
        };
    };
    let snap = match serde_json::from_str::<Value>(raw) {
        Ok(v) if v.is_object() => v,
        Ok(_) => return Report::new(Status::Unreadable, "snapshot is not a JSON object", None),
        Err(e) => {
            return Report::new(
                Status::Unreadable,
                format!("snapshot is not JSON: {e}"),
                None,
            )
        }
    };
    let pid = uint(&snap, "pid").and_then(|p| u32::try_from(p).ok());
    let stage_name = text(&snap, "stage");
    let stage = stage_name.and_then(|s| serde_json::from_value::<Stage>(Value::from(s)).ok());
    let age_ms = uint(&snap, "updated_monotonic_ms").map(|t| host.now_ms.saturating_sub(t));
    let stage_for_ms = uint(&snap, "stage_since_ms").map(|t| host.now_ms.saturating_sub(t));

    let mut report = Report::new(Status::Healthy, "listening", Some(snap.clone()));
    let mut verdict = |status: Status, summary: String| {
        report.status = status;
        report.exit_code = status.exit_code();
        report.summary = summary;
    };

    if text(&snap, "boot_id").is_some_and(|b| b != host.boot_id) {
        verdict(
            Status::NotRunning,
            "snapshot is from an earlier boot".into(),
        );
        return report;
    }
    // Times from this boot are comparable from here on.
    report.age_ms = age_ms;
    report.stage_for_ms = stage_for_ms;
    if stage == Some(Stage::Stopped) {
        verdict(Status::NotRunning, "VAD is stopped".into());
        return report;
    }
    // None: the snapshot does not say which process wrote it.
    let alive = pid.map(|pid| match (host.start_ticks)(pid) {
        None => Err(format!("VAD died without stopping (pid {pid} is gone)")),
        Some(ticks) if text(&snap, "process_start_ticks").is_some_and(|t| t != ticks) => Err(
            format!("VAD died without stopping (pid {pid} is now another process)"),
        ),
        Some(_) => Ok(()),
    });
    if let Some(Err(gone)) = alive {
        verdict(Status::NotRunning, gone);
        return report;
    }
    let Some(age_ms) = age_ms else {
        verdict(
            Status::Unreadable,
            "snapshot has no updated_monotonic_ms".into(),
        );
        return report;
    };
    let fresh = age_ms <= STALE_AFTER_MS;
    report.fresh = Some(fresh);
    if !fresh {
        if alive.is_none() {
            verdict(
                Status::NotRunning,
                format!("no update for {} and no pid to check", duration(age_ms)),
            );
            return report;
        }
        report.running = true;
        verdict(
            Status::Stuck,
            format!(
                "stale: the process is alive but has not reported for {}",
                duration(age_ms)
            ),
        );
        return report;
    }
    report.running = true;

    let asr_state = snap.get("asr").and_then(|a| text(a, "state"));
    let problem = Progress {
        stage,
        stage_since_ms: uint(&snap, "stage_since_ms").unwrap_or(host.now_ms),
        capture_last_ms: uint(&snap, "capture_last_ms"),
        detector_last_ms: uint(&snap, "detector_last_ms"),
        audio_backlog_ms: uint(&snap, "audio_backlog_ms").unwrap_or(0),
        capture_failed: text(&snap, "capture_error").is_some(),
        typing_paused: snap.get("typing_paused").and_then(Value::as_bool) == Some(true),
        asr_down: asr_state == Some("down"),
    }
    .problem(host.now_ms);

    let mut warnings = Vec::new();
    if let Some(mic) = text(&snap, "mic_warning") {
        warnings.push(mic.to_owned());
    }
    // Warnings added to the snapshot later are named the same way.
    if let Some(fields) = snap.as_object() {
        for (key, value) in fields {
            if key != "mic_warning" && key.ends_with("_warning") {
                if let Some(warning) = value.as_str().filter(|s| !s.is_empty()) {
                    warnings.push(warning.to_owned());
                }
            }
        }
    }
    if asr_state == Some("degraded") {
        warnings.push("live stream down, transcribing over HTTP".to_owned());
    }
    report.warnings = warnings;

    if let Some(problem) = problem {
        report.problem = Some(problem.to_owned());
        verdict(Status::Stuck, format!("stuck: {problem}"));
    } else if !report.warnings.is_empty() {
        let summary = report.warnings.join("; ");
        verdict(Status::Warning, summary);
    } else if let Some(stage) = stage_name {
        verdict(Status::Healthy, stage.to_owned());
    }
    report
}

/// A pid file names a live process.
pub fn pid_file_alive(pid_file: &Path) -> bool {
    std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<i32>().ok())
        .is_some_and(|pid| {
            pid > 0 && nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
        })
}

/// Read and judge the snapshot in `state_dir` against this machine.
pub fn read(state_dir: &Path) -> Report {
    let raw = match std::fs::read_to_string(state_dir.join(HEALTH_FILE)) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Report::new(
                Status::Unreadable,
                format!("cannot read snapshot: {e}"),
                None,
            )
        }
    };
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    evaluate(
        raw.as_deref(),
        &Host {
            now_ms: monotonic_ms(),
            boot_id: boot_id.trim(),
            start_ticks: &|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                process_start_ticks(&stat).ok()
            },
            pid_file_alive: pid_file_alive(&state_dir.join("vad.pid")),
        },
    )
}

fn duration(ms: u64) -> String {
    match ms {
        0..=999 => format!("{ms} ms"),
        1000..=59_999 => format!("{:.1} s", ms as f64 / 1000.0),
        60_000..=3_599_999 => format!("{} min {} s", ms / 60_000, ms % 60_000 / 1000),
        _ => format!("{} h {} min", ms / 3_600_000, ms % 3_600_000 / 60_000),
    }
}

/// The report for people. Lines whose field the snapshot lacks are left out.
pub fn render(report: &Report) -> String {
    let headline = match report.status {
        Status::Healthy => "healthy",
        Status::NotRunning => "not running",
        Status::Stuck => "stuck",
        Status::Warning => "running with warnings",
        Status::Unreadable => "unreadable",
    };
    let mut out = format!("ears vad: {headline} ({})\n", report.summary);
    let Some(snap) = &report.snapshot else {
        return out;
    };
    let mut line = |label: &str, value: String| {
        let _ = writeln!(out, "  {label:<9} {value}");
    };
    if let Some(pid) = uint(snap, "pid") {
        let state = if report.running {
            "alive"
        } else {
            "not running"
        };
        line("process", format!("pid {pid}, {state}"));
    }
    if let (Some(age), Some(fresh)) = (report.age_ms, report.fresh) {
        let state = if fresh { "fresh" } else { "stale" };
        line(
            "snapshot",
            format!("{state}, updated {} ago", duration(age)),
        );
    }
    if let Some(stage) = text(snap, "stage") {
        line(
            "stage",
            match report.stage_for_ms {
                Some(ms) => format!("{stage} for {}", duration(ms)),
                None => stage.to_owned(),
            },
        );
    }
    if let Some(device) = text(snap, "device") {
        line("device", device.to_owned());
    }
    if let Some(error) = text(snap, "capture_error") {
        line("capture", format!("stopped: {error}"));
    }
    if let Some(backlog) = uint(snap, "audio_backlog_ms") {
        line("backlog", duration(backlog));
    }
    if let Some(gain) = snap.get("gain").and_then(Value::as_f64) {
        line("gain", format!("{gain:.1}x"));
    }
    let dbfs = |key: &str| snap.get(key).and_then(Value::as_f64);
    let levels = match (dbfs("noise_dbfs"), dbfs("speech_dbfs")) {
        (Some(noise), Some(speech)) => format!(" (noise {noise:.0} dBFS, speech {speech:.0} dBFS)"),
        _ => String::new(),
    };
    match text(snap, "mic_warning") {
        Some(warning) => line("mic", format!("{warning}{levels}; try `ears mic-check`")),
        None if snap.get("mic_warning").is_some() => line("mic", format!("ok{levels}")),
        None => {}
    }
    if let Some(asr) = snap.get("asr") {
        let mut value = text(asr, "state").unwrap_or("unknown").to_owned();
        if let Some(path) = text(asr, "path") {
            let _ = write!(value, " via {path}");
        }
        if let Some(error) = text(asr, "last_error") {
            let _ = write!(value, ": {error}");
        }
        line("asr", value);
    }
    if snap.get("typing_paused").and_then(Value::as_bool) == Some(true) {
        line("typing", "paused after an output error".to_owned());
    }
    if let Some(dropped) = uint(snap, "dropped_segments") {
        line("dropped", format!("{dropped} segment(s)"));
    }
    for warning in &report.warnings {
        if text(snap, "mic_warning") != Some(warning) {
            line("warning", warning.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 43_000_000;
    const BOOT: &str = "bf995327-fcd4-48da-81df-d924e4114f91";

    /// A healthy snapshot as `ears vad` publishes it, 300 ms old.
    fn healthy() -> Value {
        json!({
            "version": 1, "pid": 4242, "process_start_ticks": "3750448",
            "boot_id": BOOT, "session_id": "4242-37504598",
            "session_kind": "desktop_vad", "device": "alsa_input.usb-mic",
            "updated_monotonic_ms": NOW - 300, "stage": "listening",
            "stage_since_ms": NOW - 12_000, "capture_last_ms": NOW - 320,
            "detector_last_ms": NOW - 310, "captured_samples": 87582400u64,
            "processed_frames": 171059, "queue_chunks": 0, "audio_backlog_ms": 12,
            "rms": 0.0034, "peak": 0.0104, "probability": 0.03, "threshold": 0.4,
            "candidate_frames": 0, "rejected_candidates": 19, "gain": 8.0,
            "dropped_segments": 1, "speaking": false, "capture_error": null,
            "typing_paused": false, "noise_dbfs": -56.5, "speech_dbfs": -27.4,
            "clipped_samples": 2, "mic_warning": null,
            "asr": {"state": "ok", "path": "stream", "last_ok_ms": 40233098, "last_error": null}
        })
    }

    fn judge_with(snap: &Value, ticks: Option<&str>) -> Report {
        let ticks = ticks.map(str::to_owned);
        evaluate(
            Some(&snap.to_string()),
            &Host {
                now_ms: NOW,
                boot_id: BOOT,
                start_ticks: &|pid| ticks.clone().filter(|_| pid == 4242),
                pid_file_alive: false,
            },
        )
    }

    fn judge(snap: &Value) -> Report {
        judge_with(snap, Some("3750448"))
    }

    #[test]
    fn healthy_snapshot_is_healthy() {
        let report = judge(&healthy());
        assert_eq!(report.status, Status::Healthy, "{report:?}");
        assert_eq!(report.exit_code, 0);
        assert!(report.running);
        assert_eq!(report.fresh, Some(true));
        assert_eq!(report.age_ms, Some(300));
        assert_eq!(report.stage_for_ms, Some(12_000));
        let text = render(&report);
        assert!(
            text.starts_with("ears vad: healthy (listening)\n"),
            "{text}"
        );
        assert!(text.contains("pid 4242, alive"), "{text}");
        assert!(text.contains("fresh, updated 300 ms ago"), "{text}");
        assert!(text.contains("listening for 12.0 s"), "{text}");
        assert!(text.contains("alsa_input.usb-mic"), "{text}");
        assert!(text.contains("gain      8.0x"), "{text}");
        assert!(text.contains("ok via stream"), "{text}");
        assert!(text.contains("1 segment(s)"), "{text}");
    }

    #[test]
    fn stale_snapshot_of_a_live_process_is_stuck() {
        let mut snap = healthy();
        snap["updated_monotonic_ms"] = json!(NOW - 42_000);
        let report = judge(&snap);
        assert_eq!(report.status, Status::Stuck);
        assert_eq!(report.exit_code, 2);
        assert!(report.running);
        assert_eq!(report.fresh, Some(false));
        assert!(report.summary.contains("42.0 s"), "{}", report.summary);
        assert!(render(&report).contains("stale, updated 42.0 s ago"));
    }

    #[test]
    fn dead_or_recycled_pid_is_not_running() {
        // Killed: the last snapshot still says "listening".
        let report = judge_with(&healthy(), None);
        assert_eq!(report.status, Status::NotRunning);
        assert_eq!(report.exit_code, 1);
        assert!(!report.running);
        assert!(report.summary.contains("pid 4242 is gone"));
        // The pid exists again, as some other process.
        let report = judge_with(&healthy(), Some("9999999"));
        assert_eq!(report.status, Status::NotRunning);
        assert!(report.summary.contains("another process"));
    }

    #[test]
    fn stopped_missing_and_other_boot_are_not_running() {
        let mut snap = healthy();
        snap["stage"] = json!("stopped");
        assert_eq!(judge(&snap).status, Status::NotRunning);
        let mut snap = healthy();
        snap["boot_id"] = json!("another-boot");
        let report = judge(&snap);
        assert_eq!(report.status, Status::NotRunning);
        assert_eq!(report.age_ms, None, "times of another boot mean nothing");
        let host = |pid_file_alive| Host {
            now_ms: NOW,
            boot_id: BOOT,
            start_ticks: &|_| None,
            pid_file_alive,
        };
        assert_eq!(evaluate(None, &host(false)).status, Status::NotRunning);
        assert_eq!(evaluate(None, &host(true)).status, Status::Stuck);
    }

    #[test]
    fn warnings_keep_running_but_exit_3() {
        let mut snap = healthy();
        snap["mic_warning"] = json!("speech too quiet");
        let report = judge(&snap);
        assert_eq!(report.status, Status::Warning);
        assert_eq!(report.exit_code, 3);
        assert_eq!(report.warnings, ["speech too quiet"]);
        assert!(render(&report).contains("speech too quiet (noise -56 dBFS"));

        let mut snap = healthy();
        snap["asr"] = json!({"state": "degraded", "path": "http"});
        let report = judge(&snap);
        assert_eq!(report.status, Status::Warning);
        assert!(render(&report).contains("degraded via http"));
    }

    #[test]
    fn a_reported_problem_is_stuck_and_outranks_warnings() {
        let mut snap = healthy();
        snap["mic_warning"] = json!("mic silent");
        snap["capture_last_ms"] = json!(NOW - 4000);
        let report = judge(&snap);
        assert_eq!(report.status, Status::Stuck);
        assert_eq!(report.problem.as_deref(), Some("no audio arriving"));
        assert_eq!(report.warnings, ["mic silent"]);

        let mut snap = healthy();
        snap["asr"] = json!({"state": "down", "last_error": "connection refused"});
        let report = judge(&snap);
        assert_eq!(
            report.problem.as_deref(),
            Some("transcription server unreachable")
        );
        assert!(render(&report).contains("down: connection refused"));
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        // Fields a newer writer adds: passed through; a `*_warning` counts.
        let mut snap = healthy();
        snap["device_warning"] = json!("configured device missing, using the default");
        snap["some_future_field"] = json!({"nested": [1, 2, 3]});
        snap["stage"] = json!("a_future_stage");
        let report = judge(&snap);
        assert_eq!(report.status, Status::Warning, "{report:?}");
        assert_eq!(
            report.warnings,
            ["configured device missing, using the default"]
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["snapshot"]["some_future_field"]["nested"][2], 3);
        assert_eq!(json["status"], "warning");
        assert!(render(&report).contains("a_future_stage for"));

        // An older or thinner writer: only identity and the update time.
        let thin = json!({"pid": 4242, "updated_monotonic_ms": NOW - 100});
        let report = judge(&thin);
        assert_eq!(report.status, Status::Healthy, "{report:?}");
        assert_eq!(
            render(&report).lines().count(),
            3,
            "headline, process, snapshot"
        );

        for bad in ["", "[1]", "{\"pid\": 4242}"] {
            let report = evaluate(
                Some(bad),
                &Host {
                    now_ms: NOW,
                    boot_id: BOOT,
                    start_ticks: &|_| Some("1".into()),
                    pid_file_alive: false,
                },
            );
            assert_eq!(report.status, Status::Unreadable, "{bad}");
            assert_eq!(report.exit_code, 4);
        }
    }

    #[test]
    fn reads_what_the_monitor_writes() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()).status, Status::NotRunning);
        let monitor = crate::health::HealthMonitor::start(dir.path()).unwrap();
        monitor.health().captured(&[0.0; 512]);
        monitor.health().frame(0.0, 0.5, 0, false, None);
        let report = read(dir.path());
        assert!(report.running, "{report:?}");
        assert_eq!(report.fresh, Some(true));
        drop(monitor);
        let report = read(dir.path());
        assert_eq!(report.status, Status::NotRunning);
        assert_eq!(report.summary, "VAD is stopped");
    }
}
