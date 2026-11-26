#!/usr/bin/env python3
"""ears-metrics.py — Compute real-time Ears Responsiveness Index (ERI) & latency
progress vs baseline from $XDG_RUNTIME_DIR/ears/debug.log.

Outputs atomic JSON to $XDG_RUNTIME_DIR/ears/responsiveness.json for the
HAIos Quickshell progress plot, and persists milestone snapshots in
~/.local/state/ears/responsiveness-history.json.

Usage:
  ears-metrics.py                  # update responsiveness.json once
  ears-metrics.py --json           # update and print JSON to stdout
  ears-metrics.py snapshot "label" # pin current live stats as a milestone
  ears-metrics.py --watch          # update continuously when debug.log changes
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

XDG_RUNTIME = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
XDG_STATE = Path(os.environ.get("XDG_STATE_HOME") or (Path.home() / ".local/state"))

LOG_PATH = Path(os.environ.get("EARS_DEBUG_LOG") or (XDG_RUNTIME / "ears/debug.log"))
OUT_PATH = Path(os.environ.get("EARS_METRICS_OUT") or (XDG_RUNTIME / "ears/responsiveness.json"))
HISTORY_PATH = Path(
    os.environ.get("EARS_METRICS_HISTORY")
    or (XDG_STATE / "ears/responsiveness-history.json")
)

# Real measured baseline on centurion (2026-11-26 11:35, ears v1.2.0, n=110)
# before perf/responsive-ux optimizations.
BASELINE = {
    "id": "baseline-v1.2.0",
    "label": "Baseline (v1.2.0)",
    "short": "M0 BASE",
    "timestamp": "2026-11-26T11:35:00+01:00",
    "n_utterances": 110,
    "dictation_silence_med_ms": 421.0,
    "dictation_asr_med_ms": 111.5,
    "dictation_post_med_ms": 59.0,
    "dictation_proc_med_ms": 186.4,
    "dictation_e2e_med_ms": 607.4,
    "command_silence_med_ms": 400.0,
    "command_asr_med_ms": 115.0,
    "command_exec_med_ms": 252.0,
    "command_proc_med_ms": 367.0,
    "command_e2e_med_ms": 767.0,
    "asr_p95_ms": 262.1,
    "silence_p95_ms": 1500.0,
    "partial_commit_pct": 0.0,
    "micro": {
        "voice_cmd_ms": 42.30,
        "hypr_ipc_ms": 3.62,
        "capture_chunk_ms": 100.0,
    },
}

# Human-perceived "instant" floor for voice-to-action reaction (ms)
INSTANT_FLOOR_E2E_MS = 150.0
INSTANT_FLOOR_PROC_MS = 5.0

RE_TS = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)")
RE_END = re.compile(r"Utterance ended after (\d+) ms of silence \(([^)]+)\)")
RE_TR = re.compile(r"Transcription took ([\d.]+)(ns|µs|us|ms|s)\b")
RE_SEG = re.compile(r"Segment #(\d+) total: ([\d.]+)(ns|µs|us|ms|s)\b")
RE_CMD = re.compile(
    r"Command timing: asr (\d+) ms, decision (\d+) ms, action (\d+) ms after (\d+) ms of silence \(([^)]+)\)"
)
RE_STREAM_FIN = re.compile(r"The live stream's final came in (\d+) ms")
RE_PARTIAL_COMMIT = re.compile(r"Committing the stream's last partial")
RE_WHISPER_HTTP = re.compile(r"Whisper API call completed in ([\d.]+)(ns|µs|us|ms|s)\b")
RE_TRANSCRIBED = re.compile(r"ears::streaming_engine: Transcribed: (.*)$")


def to_ms(val: str, unit: str) -> float:
    x = float(val)
    if unit == "ns":
        return x / 1_000_000.0
    if unit in ("µs", "us"):
        return x / 1_000.0
    if unit == "s":
        return x * 1_000.0
    return x


def med(arr: list[float]) -> float | None:
    return round(statistics.median(arr), 2) if arr else None


def pct(arr: list[float], p: float) -> float | None:
    if not arr:
        return None
    s = sorted(arr)
    idx = min(len(s) - 1, max(0, int(round((len(s) - 1) * p))))
    return round(s[idx], 2)


def compute_scores(dict_e2e: float | None, cmd_e2e: float | None, dict_proc: float | None, cmd_proc: float | None) -> dict:
    base_e2e = 0.5 * BASELINE["dictation_e2e_med_ms"] + 0.5 * BASELINE["command_e2e_med_ms"]
    base_proc = 0.5 * BASELINE["dictation_proc_med_ms"] + 0.5 * BASELINE["command_proc_med_ms"]

    e2e_parts = [v for v in (dict_e2e, cmd_e2e) if v is not None]
    proc_parts = [v for v in (dict_proc, cmd_proc) if v is not None]
    cur_e2e = sum(e2e_parts) / len(e2e_parts) if e2e_parts else base_e2e
    cur_proc = sum(proc_parts) / len(proc_parts) if proc_parts else base_proc

    e2e_score = max(0.0, min(100.0, 100.0 * (base_e2e - cur_e2e) / max(1.0, base_e2e - INSTANT_FLOOR_E2E_MS)))
    proc_score = max(0.0, min(100.0, 100.0 * (base_proc - cur_proc) / max(1.0, base_proc - INSTANT_FLOOR_PROC_MS)))

    return {
        "baseline_combined_e2e_ms": round(base_e2e, 1),
        "current_combined_e2e_ms": round(cur_e2e, 1),
        "baseline_combined_proc_ms": round(base_proc, 1),
        "current_combined_proc_ms": round(cur_proc, 1),
        "e2e_speedup": round(base_e2e / max(1.0, cur_e2e), 2),
        "proc_speedup": round(base_proc / max(0.5, cur_proc), 2),
        "e2e_reduction_pct": round(100.0 * (base_e2e - cur_e2e) / base_e2e, 1),
        "proc_reduction_pct": round(100.0 * (base_proc - cur_proc) / base_proc, 1),
        "responsiveness_score": round(e2e_score, 1),
        "processing_score": round(proc_score, 1),
        "instant_floor_e2e_ms": INSTANT_FLOOR_E2E_MS,
        "instant_floor_proc_ms": INSTANT_FLOOR_PROC_MS,
    }


def parse_log(log_path: Path, since_iso: str = "2026-11-26T12:03:32Z", max_points: int = 80) -> list[dict]:
    if not log_path.exists():
        return []
    events: list[dict] = []
    cur: dict | None = None

    with log_path.open("r", errors="replace") as f:
        for line in f:
            ts_m = RE_TS.match(line)
            ts = ts_m.group(1) if ts_m else ""
            if ts and ts < since_iso:
                continue

            if m := RE_END.search(line):
                cur = {
                    "ts": ts,
                    "silence_ms": float(m.group(1)),
                    "end_reason": m.group(2),
                    "asr_ms": None,
                    "path": "stream",
                    "partial_commit": False,
                    "text": "",
                }
                continue

            if cur is None:
                continue

            if RE_PARTIAL_COMMIT.search(line):
                cur["partial_commit"] = True
                cur["path"] = "partial"
            elif RE_WHISPER_HTTP.search(line):
                cur["path"] = "http"
            elif RE_STREAM_FIN.search(line):
                cur["path"] = "stream"

            if m := RE_TR.search(line):
                cur["asr_ms"] = round(to_ms(m.group(1), m.group(2)), 2)

            if m := RE_TRANSCRIBED.search(line):
                cur["text"] = m.group(1).strip()[:48]

            if m := RE_CMD.search(line):
                asr = float(m.group(1))
                dec = float(m.group(2))
                act = float(m.group(3))
                sil = float(m.group(4))
                label = m.group(5)
                events.append(
                    {
                        "idx": len(events) + 1,
                        "ts": ts or cur["ts"],
                        "kind": "command",
                        "label": label,
                        "end_reason": cur["end_reason"],
                        "path": cur["path"],
                        "partial_commit": cur["partial_commit"],
                        "silence_ms": round(sil, 1),
                        "asr_ms": round(asr, 2),
                        "post_ms": round(max(0.0, act - dec), 2),
                        "proc_ms": round(act, 2),
                        "e2e_ms": round(sil + act, 1),
                    }
                )
                # Tail commands also emit a Segment #N total line immediately after;
                # clear cur so we don't double-count the same utterance.
                cur = None
                continue

            if m := RE_SEG.search(line):
                proc = to_ms(m.group(2), m.group(3))
                asr = cur["asr_ms"] if cur["asr_ms"] is not None else proc
                post = max(0.0, proc - asr)
                sil = cur["silence_ms"]
                events.append(
                    {
                        "idx": len(events) + 1,
                        "ts": ts or cur["ts"],
                        "kind": "dictation",
                        "label": cur["text"] or f"seg #{m.group(1)}",
                        "end_reason": cur["end_reason"],
                        "path": cur["path"],
                        "partial_commit": cur["partial_commit"],
                        "silence_ms": round(sil, 1),
                        "asr_ms": round(asr, 2),
                        "post_ms": round(post, 2),
                        "proc_ms": round(proc, 2),
                        "e2e_ms": round(sil + proc, 1),
                    }
                )
                cur = None

    return events[-max_points:]


def probe_live_micro() -> dict:
    """Measure live hot-path telemetry directly instead of hardcoding constants."""
    import socket

    hypr_ipc_ms: float | None = None
    sig = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE")
    if sig:
        candidates = []
        if os.environ.get("XDG_RUNTIME_DIR"):
            candidates.append(Path(os.environ["XDG_RUNTIME_DIR"]) / "hypr" / sig / ".socket.sock")
        candidates.append(Path("/tmp/hypr") / sig / ".socket.sock")
        for sock_path in candidates:
            if not sock_path.exists():
                continue
            try:
                t0 = time.perf_counter()
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
                    s.settimeout(0.1)
                    s.connect(str(sock_path))
                    s.sendall(b"j/activewindow")
                    while s.recv(4096):
                        pass
                hypr_ipc_ms = round((time.perf_counter() - t0) * 1000.0, 2)
                break
            except OSError:
                continue

    steer_resident = False
    capture_chunk_ms: float | None = None
    proc = Path("/proc")
    if proc.is_dir():
        for entry in proc.iterdir():
            if not entry.name.isdigit():
                continue
            try:
                raw = (entry / "cmdline").read_bytes()
            except OSError:
                continue
            if not raw:
                continue
            args = [a.decode("utf-8", "ignore") for a in raw.split(b"\0") if a]
            if not args:
                continue
            joined = " ".join(args)
            if "voice-cmd" in joined and "serve" in args:
                steer_resident = True
            if Path(args[0]).name == "pw-record":
                for arg in args[1:]:
                    if arg.startswith("--latency="):
                        val = arg.split("=", 1)[1].strip()
                        if val.endswith("ms"):
                            try:
                                capture_chunk_ms = float(val[:-2])
                            except ValueError:
                                pass

    return {
        "hypr_ipc_ms": hypr_ipc_ms,
        "steer_resident": steer_resident,
        "capture_chunk_ms": capture_chunk_ms,
    }


def summarize(events: list[dict], label: str = "Live (perf/responsive-ux)", short: str = "LIVE") -> dict:
    dicts = [e for e in events if e["kind"] == "dictation"]
    cmds = [e for e in events if e["kind"] == "command"]
    all_asr = [e["asr_ms"] for e in events if e["asr_ms"] is not None]
    all_sil = [e["silence_ms"] for e in events]

    d_sil = med([e["silence_ms"] for e in dicts])
    d_asr = med([e["asr_ms"] for e in dicts])
    d_post = med([e["post_ms"] for e in dicts])
    d_proc = med([e["proc_ms"] for e in dicts])
    d_e2e = med([e["e2e_ms"] for e in dicts])

    c_sil = med([e["silence_ms"] for e in cmds])
    c_asr = med([e["asr_ms"] for e in cmds])
    c_exec = med([e["post_ms"] for e in cmds])
    c_proc = med([e["proc_ms"] for e in cmds])
    c_e2e = med([e["e2e_ms"] for e in cmds])

    partial_pct = (
        round(100.0 * sum(1 for e in events if e["partial_commit"]) / len(events), 1)
        if events
        else 0.0
    )

    scores = compute_scores(d_e2e, c_e2e, d_proc, c_proc)

    return {
        "label": label,
        "short": short,
        "timestamp": datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds"),
        "n_utterances": len(events),
        "n_dictation": len(dicts),
        "n_command": len(cmds),
        "dictation_silence_med_ms": d_sil,
        "dictation_asr_med_ms": d_asr,
        "dictation_post_med_ms": d_post,
        "dictation_proc_med_ms": d_proc,
        "dictation_e2e_med_ms": d_e2e,
        "command_silence_med_ms": c_sil,
        "command_asr_med_ms": c_asr,
        "command_exec_med_ms": c_exec,
        "command_proc_med_ms": c_proc,
        "command_e2e_med_ms": c_e2e,
        "asr_p95_ms": pct(all_asr, 0.95),
        "silence_p95_ms": pct(all_sil, 0.95),
        "partial_commit_pct": partial_pct,
        "micro": probe_live_micro(),
        **scores,
    }


def load_history() -> list[dict]:
    if HISTORY_PATH.exists():
        try:
            data = json.loads(HISTORY_PATH.read_text())
            if isinstance(data, list) and data:
                return data
        except Exception:
            pass
    base_scores = compute_scores(
        BASELINE["dictation_e2e_med_ms"],
        BASELINE["command_e2e_med_ms"],
        BASELINE["dictation_proc_med_ms"],
        BASELINE["command_proc_med_ms"],
    )
    return [{**BASELINE, **base_scores}]


def save_history(milestones: list[dict]) -> None:
    HISTORY_PATH.parent.mkdir(parents=True, exist_ok=True)
    tmp = HISTORY_PATH.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(milestones, indent=2) + "\n")
    tmp.replace(HISTORY_PATH)


def build_payload(snapshot_label: str | None = None) -> dict:
    events = parse_log(LOG_PATH)
    live = summarize(events)
    milestones = load_history()
    if snapshot_label:
        snap = dict(live)
        snap["id"] = f"snap-{int(time.time())}"
        snap["label"] = snapshot_label
        snap["short"] = f"M{len(milestones)}"
        milestones.append(snap)
        save_history(milestones)
    elif not HISTORY_PATH.exists():
        save_history(milestones)

    # Rolling 5-utterance median for smooth trend overlay
    rolling_e2e: list[float] = []
    rolling_proc: list[float] = []
    for i in range(len(events)):
        win = events[max(0, i - 4) : i + 1]
        rolling_e2e.append(round(statistics.median(e["e2e_ms"] for e in win), 1))
        rolling_proc.append(round(statistics.median(e["proc_ms"] for e in win), 2))
        events[i]["rolling_e2e_ms"] = rolling_e2e[-1]
        events[i]["rolling_proc_ms"] = rolling_proc[-1]

    health = {}
    health_path = XDG_RUNTIME / "ears/vad-health.json"
    if health_path.exists():
        try:
            h = json.loads(health_path.read_text())
            health = {
                "stage": h.get("stage"),
                "device": h.get("device_opened") or h.get("device"),
                "asr_path": (h.get("asr") or {}).get("path"),
                "asr_state": (h.get("asr") or {}).get("state"),
                "backlog_ms": h.get("audio_backlog_ms", 0),
                "mean_prob": round(float(h.get("last_segment_mean_probability") or 0.0), 2),
            }
        except Exception:
            pass

    return {
        "updated_at": datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds"),
        "baseline": milestones[0],
        "milestones": milestones,
        "live": live,
        "events": events,
        "health": health,
    }


def write_atomic(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(payload, separators=(",", ":")) + "\n")
    tmp.replace(path)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", nargs="?", choices=["snapshot"], help="Optional subcommand")
    ap.add_argument("label", nargs="?", help="Label for snapshot")
    ap.add_argument("--json", action="store_true", help="Print JSON payload to stdout")
    ap.add_argument("--watch", action="store_true", help="Watch debug.log and refresh continuously")
    args = ap.parse_args()

    if args.watch:
        last_sig = None
        while True:
            try:
                st = LOG_PATH.stat()
                sig = (st.st_mtime_ns, st.st_size)
            except OSError:
                sig = None
            if sig != last_sig:
                payload = build_payload()
                write_atomic(OUT_PATH, payload)
                last_sig = sig
            time.sleep(0.8)

    snap_label = (args.label or f"Checkpoint {datetime.now().strftime('%H:%M')}") if args.cmd == "snapshot" else None
    payload = build_payload(snapshot_label=snap_label)
    write_atomic(OUT_PATH, payload)
    if args.json:
        print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
