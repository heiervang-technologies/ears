#!/usr/bin/env python3
"""Does passing the active command phrases as ASR context help?

Speaks every phrase in commands.txt and every sentence in ordinary.txt with
espeak-ng, decodes each clip under several contexts and reports exact-match
rate, false positives and latency. See README.md for the conditions.

    python3 -I eval.py run   [--server URL] [--wavs DIR] [--out results.jsonl]
    python3 -I eval.py table [--out results.jsonl]

Never uses `say` (it would speak aloud). Requests: at most --budget, two in
flight at most: the ASR serves live voice typing.
"""

import argparse
import base64
import json
import random
import re
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
MODEL = "Qwen/Qwen3-ASR-1.7B"
VOICES = {
    "us150": ["-v", "en-us", "-s", "150"],
    "gb175": ["-v", "en-gb", "-s", "175", "-p", "40"],
}
CAPS = [256, 1024, 4096]


def normalize(text: str) -> str:
    """Same as ears' commands::normalize."""
    return " ".join(re.sub(r"[^\w]|_", " ", text.lower()).split())


def load(name: str) -> list[list[str]]:
    rows = []
    for line in (HERE / name).read_text().splitlines():
        if line.strip() and not line.startswith("#"):
            rows.append([f.strip() for f in line.split("|")])
    return rows


def filler(exclude: set[str]) -> list[str]:
    """Other command phrasings, to grow a context to the larger caps.

    Shaped like a real actions.toml: workspaces, tmux, window moves.
    """
    words = "one two three four five six seven eight nine ten".split()
    out = []
    for i, w in enumerate(words, 1):
        out += [f"workspace {w}", f"switch to workspace {w}", f"move window to workspace {w}",
                f"move window silently to workspace {w}", f"tmux window {w}",
                f"tmux select window {w}", f"switch to group window {w}", f"super shift {w}"]
    for d in ["left", "right", "up", "down"]:
        out += [f"go {d}", f"arrow {d}", f"move {d}", f"move window {d}", f"focus on {d} window",
                f"swap window {d}", f"expand window {d}", f"resize {d}", f"tmux pane {d}",
                f"tmux resize the pane {d}", f"scroll {d}", f"move workspace to {d} monitor",
                f"move window to group on {d}", f"super alt {d}"]
    out += ["press enter", "submit", "send it", "confirm", "escape", "cancel", "never mind",
            "dismiss", "tab", "next field", "backspace", "delete a letter", "erase", "play",
            "resume playback", "skip this song", "last song", "louder", "quieter", "volume down",
            "turn it up", "zoom out", "zoom reset", "magnify", "scroll to the bottom",
            "keep scrolling up", "keep scrolling down", "pan up", "pan down", "pan right",
            "stop", "halt", "voronoi picker", "show targets", "hide commands", "tmux break pane",
            "tmux rename current session", "tmux list key bindings", "tmux new window",
            "tmux paste", "tmux zoom", "tmux tile", "tmux choose tree", "tmux source file",
            "tmux run shell", "tmux last pane", "tmux swap pane", "tmux join pane", "mute",
            "brightness down", "brightness maximum", "brightness minimum", "toggle touchpad",
            "universal copy", "universal paste", "clipboard manager", "float window",
            "tile window", "full width", "cycle windows", "calculator", "file manager",
            "screenshot full screen", "show weather", "wifi controls", "toggle top bar",
            "switch keyboard layout", "toggle vibe mode", "clear reminders", "capture menu",
            "restore last notification", "toggle laptop display", "music tui", "inventory"]
    seen, uniq = set(exclude), []
    for p in out:
        if normalize(p) not in seen:
            seen.add(normalize(p))
            uniq.append(p)
    return uniq


def capped(phrases: list[str], cap: int) -> str:
    """Whole phrases, comma-joined, at most `cap` characters."""
    out = ""
    for p in phrases:
        nxt = p if not out else f"{out}, {p}"
        if len(nxt) > cap:
            break
        out = nxt
    return out


def contexts(commands: list[list[str]]) -> dict[str, str]:
    spoken = [c[0] for c in commands]
    universe = list(spoken) + filler({normalize(f) for c in commands for f in c})
    rng = random.Random(7)
    mixed = universe[:]
    rng.shuffle(mixed)
    ctx = {"none": "", "list": ", ".join(spoken)}
    for cap in CAPS:
        ctx[f"cap{cap}"] = capped(mixed, cap)
    return ctx


def tts(text: str, voice: str, wavs: Path) -> Path:
    slug = re.sub(r"\W+", "-", text.lower()).strip("-")[:60]
    out = wavs / f"{voice}-{slug}.wav"
    if not out.exists():
        raw = out.with_suffix(".raw.wav")
        subprocess.run(["espeak-ng", *VOICES[voice], "-w", str(raw), text], check=True)
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(raw), "-ar", "16000",
                        "-ac", "1", str(out)], check=True)
        raw.unlink()
    return out


class Asr:
    def __init__(self, server: str, budget: int):
        self.server = server.rstrip("/")
        self.budget = budget
        self.sent = 0
        self.lock = threading.Lock()

    def _take(self):
        with self.lock:
            if self.sent >= self.budget:
                raise RuntimeError(f"request budget of {self.budget} spent")
            self.sent += 1

    def http(self, wav: Path, context: str) -> tuple[str, float]:
        """/v1/audio/transcriptions with `prompt`, as ears' WhisperClient."""
        self._take()
        boundary = "csctx"
        parts = [("response_format", "json")] + ([("prompt", context)] if context else [])
        body = b""
        for k, v in parts:
            body += (f"--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n"
                     f"{v}\r\n").encode()
        body += (f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; "
                 f"filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n").encode()
        body += wav.read_bytes() + f"\r\n--{boundary}--\r\n".encode()
        req = urllib.request.Request(f"{self.server}/v1/audio/transcriptions", body,
                                     {"Content-Type": f"multipart/form-data; boundary={boundary}"})
        t = time.monotonic()
        text = json.load(urllib.request.urlopen(req, timeout=60))["text"]
        return text, time.monotonic() - t

    def chat(self, wav: Path, context: str) -> tuple[str, float]:
        """Chat completions with the context as the system turn, as the live decoder."""
        self._take()
        audio = base64.b64encode(wav.read_bytes()).decode()
        messages = [{"role": "system", "content": context}] if context else []
        messages.append({"role": "user", "content": [
            {"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]})
        body = {"model": MODEL, "messages": messages, "max_tokens": 96, "temperature": 0.0}
        req = urllib.request.Request(f"{self.server}/v1/chat/completions",
                                     json.dumps(body).encode(),
                                     {"Content-Type": "application/json"})
        t = time.monotonic()
        out = json.load(urllib.request.urlopen(req, timeout=60))["choices"][0]["message"]["content"]
        return out.split("<asr_text>", 1)[-1].strip(), time.monotonic() - t


NOISES = {
    "silence": ["-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono", "-t", "1.5"],
    "white-quiet": ["-f", "lavfi", "-i", "anoisesrc=r=16000:c=white:a=0.01", "-t", "1.5"],
    "white-loud": ["-f", "lavfi", "-i", "anoisesrc=r=16000:c=white:a=0.2", "-t", "1.5"],
    "pink": ["-f", "lavfi", "-i", "anoisesrc=r=16000:c=pink:a=0.1", "-t", "1.5"],
    "brown": ["-f", "lavfi", "-i", "anoisesrc=r=16000:c=brown:a=0.3", "-t", "1.5"],
    "hum": ["-f", "lavfi", "-i", "sine=f=100:r=16000", "-af", "volume=0.2", "-t", "1.5"],
    "beep": ["-f", "lavfi", "-i", "sine=f=880:r=16000:d=0.3", "-af", "apad=pad_dur=0.7"],
    "short-white": ["-f", "lavfi", "-i", "anoisesrc=r=16000:c=white:a=0.1", "-t", "0.4"],
}


NOISE_COLS = [("http", "none"), ("http", "list"), ("http", "cap1024"), ("chat", "none"),
              ("chat", "list"), ("forced", "list")]


def noise(name: str, wavs: Path) -> Path:
    out = wavs / f"noise-{name}.wav"
    if not out.exists():
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", *NOISES[name], "-ar", "16000",
                        "-ac", "1", "-c:a", "pcm_s16le", str(out)], check=True)
    return out


def chat_template() -> str:
    """The template ears' continuous decoder sends (src/continuous.rs)."""
    src = (HERE.parent.parent / "src/continuous.rs").read_text()
    body = src.split("const CHAT_TEMPLATE: &str = concat!(", 1)[1].split(");", 1)[0]
    parts = re.findall(r'^\s*"((?:[^"\\]|\\.)*)",\s*$', body, re.M)
    return "".join(json.loads(f'"{p}"') for p in parts)


def chat_forced(asr: "Asr", wav: Path, context: str) -> tuple[str, float]:
    """Chat with the language header forced, as a pinned-language decoder."""
    asr._take()
    audio = base64.b64encode(wav.read_bytes()).decode()
    messages = [{"role": "system", "content": context}] if context else []
    messages += [{"role": "user", "content": [
        {"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]},
        {"role": "assistant", "content": "language English<asr_text>"}]
    body = {"model": MODEL, "messages": messages, "max_tokens": 96, "temperature": 0.0,
            "add_generation_prompt": False, "continue_final_message": True,
            "chat_template": chat_template()}
    req = urllib.request.Request(f"{asr.server}/v1/chat/completions", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    t = time.monotonic()
    out = json.load(urllib.request.urlopen(req, timeout=60))["choices"][0]["message"]["content"]
    return out.split("<asr_text>", 1)[-1].strip(), time.monotonic() - t


def plan2(first: Path) -> list[dict]:
    """Second run: non-speech clips, and the forced header on the clips the
    context emptied in the first run."""
    commands = load("commands.txt")
    ctx = contexts(commands)
    jobs = []
    for name in NOISES:
        for path, conds in (("http", ("none", "list", "cap1024")), ("chat", ("none", "list")),
                            ("forced", ("list",))):
            for cond in conds:
                jobs.append({"path": path, "cond": cond, "voice": "noise", "kind": "noise",
                             "forms": [name], "context": ctx[cond]})
    emptied = []
    for line in open(first):
        r = json.loads(line)
        if (r["path"] == "http" and r["voice"] == "us150" and r["kind"] == "command"
                and r["cond"] == "list" and not normalize(r["heard"])):
            emptied.append(r["forms"])
    for forms in emptied:
        jobs.append({"path": "forced", "cond": "list", "voice": "us150", "kind": "command",
                     "forms": forms, "context": ctx["list"]})
    return jobs


def run2(args):
    wavs = Path(args.wavs)
    jobs = plan2(Path(args.out))
    print(f"{len(jobs)} requests planned", file=sys.stderr)
    asr = Asr(args.server, args.budget)

    def one(job):
        wav = (noise(job["forms"][0], wavs) if job["kind"] == "noise"
               else tts(job["forms"][0], job["voice"], wavs))
        fn = {"http": asr.http, "chat": asr.chat,
              "forced": lambda w, c: chat_forced(asr, w, c)}[job["path"]]
        text, dt = fn(wav, job["context"])
        return {**{k: v for k, v in job.items() if k != "context"},
                "context_chars": len(job["context"]), "heard": text, "seconds": round(dt, 4)}

    for job in jobs:
        if job["kind"] == "noise":
            noise(job["forms"][0], wavs)
    with ThreadPoolExecutor(max_workers=2) as pool, open(args.out2, "w") as f:
        for r in pool.map(one, jobs):
            f.write(json.dumps(r) + "\n")
            print(f"{r['path']:6} {r['cond']:8} {r['forms'][0]:28} -> {r['heard']!r}")
    print(f"{asr.sent} requests sent", file=sys.stderr)


def plan() -> list[dict]:
    commands, ordinary = load("commands.txt"), load("ordinary.txt")
    ctx = contexts(commands)
    clips = [("command", c) for c in commands] + [("ordinary", o) for o in ordinary]
    jobs = []
    for cond in ctx:
        voices = ["us150", "gb175"] if cond in ("none", "list") else ["us150"]
        for voice in voices:
            for kind, forms in clips:
                jobs.append({"path": "http", "cond": cond, "voice": voice, "kind": kind,
                             "forms": forms, "context": ctx[cond]})
    # Parity: the live decoder's path (system turn) on the command clips.
    for cond in ("none", "list"):
        for forms in commands[:24]:
            jobs.append({"path": "chat", "cond": cond, "voice": "us150", "kind": "command",
                         "forms": forms, "context": ctx[cond]})
    return jobs


def run(args):
    wavs = Path(args.wavs)
    wavs.mkdir(parents=True, exist_ok=True)
    jobs = plan()
    print(f"{len(jobs)} requests planned", file=sys.stderr)
    asr = Asr(args.server, args.budget)

    def one(job):
        wav = tts(job["forms"][0], job["voice"], wavs)
        fn = asr.http if job["path"] == "http" else asr.chat
        text, dt = fn(wav, job["context"])
        return {**{k: v for k, v in job.items() if k != "context"},
                "context_chars": len(job["context"]), "heard": text, "seconds": round(dt, 4)}

    # TTS first, serially, so request latency is not mixed with synthesis.
    for job in jobs:
        tts(job["forms"][0], job["voice"], wavs)
    with ThreadPoolExecutor(max_workers=2) as pool, open(args.out, "w") as f:
        for r in pool.map(one, jobs):
            f.write(json.dumps(r) + "\n")
    print(f"{asr.sent} requests sent", file=sys.stderr)


def pct(n, d):
    return f"{100 * n / d:.0f}% ({n}/{d})" if d else "-"


def table(args):
    rows = [json.loads(l) for l in open(args.out)]
    commands = load("commands.txt")
    ctx = contexts(commands)
    universe = {normalize(f) for c in commands for f in c}
    universe |= {normalize(p) for p in filler(set(universe))}
    print("| path | condition | voice | ctx chars | cmd exact | in ctx | not in ctx | "
          "misroute | ordinary FP | ordinary exact | empty | median ms | p90 ms |")
    print("|" + "---|" * 13)
    # "+fallback": what ears does. An empty or echoed result with a context
    # is decoded again without it (the "none" row, same clip and voice:
    # temperature 0), and both latencies count.
    base = {(r["path"], r["voice"], r["kind"], r["forms"][0]): r
            for r in rows if r["cond"] == "none"}
    derived = []
    for r in rows:
        if r["cond"] == "none":
            continue
        b = base.get((r["path"], r["voice"], r["kind"], r["forms"][0]))
        if b is None:
            continue
        d = dict(r, cond=r["cond"] + "+fallback")
        if not normalize(r["heard"]):
            d["heard"] = b["heard"]
            d["seconds"] = r["seconds"] + b["seconds"]
        derived.append(d)
    rows = rows + derived
    for cond in list(ctx):
        ctx[cond + "+fallback"] = ctx[cond]
    keys = []
    for r in rows:
        k = (r["path"], r["cond"], r["voice"])
        if k not in keys:
            keys.append(k)
    for k in keys:
        rs = [r for r in rows if (r["path"], r["cond"], r["voice"]) == k]
        in_ctx = {normalize(p) for p in ctx[k[1]].split(", ")} if ctx[k[1]] else set()
        cmd = [r for r in rs if r["kind"] == "command"]
        ok = [r for r in cmd if normalize(r["heard"]) in {normalize(f) for f in r["forms"]}]
        inside = [r for r in cmd if normalize(r["forms"][0]) in in_ctx]
        outside = [r for r in cmd if normalize(r["forms"][0]) not in in_ctx]
        mis = [r for r in cmd if r not in ok and normalize(r["heard"]) in universe]
        ordn = [r for r in rs if r["kind"] == "ordinary"]
        fp = [r for r in ordn if normalize(r["heard"]) in universe]
        oex = [r for r in ordn if normalize(r["heard"]) == normalize(r["forms"][0])]
        empty = [r for r in rs if not normalize(r["heard"])]
        ms = sorted(1000 * r["seconds"] for r in rs)
        p90 = ms[int(0.9 * (len(ms) - 1))]
        print(f"| {k[0]} | {k[1]} | {k[2]} | {len(ctx[k[1]])} | {pct(len(ok), len(cmd))} | "
              f"{pct(sum(r in ok for r in inside), len(inside))} | "
              f"{pct(sum(r in ok for r in outside), len(outside))} | {len(mis)} | "
              f"{pct(len(fp), len(ordn))} | {pct(len(oex), len(ordn))} | {len(empty)} | "
              f"{statistics.median(ms):.0f} | {p90:.0f} |")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("cmd", choices=["run", "run2", "table", "noise", "contexts"])
    p.add_argument("--out2", default=str(HERE / "results-noise.jsonl"))
    p.add_argument("--server", default="http://localhost:30189")
    p.add_argument("--wavs", default="/tmp/cs-context-wavs")
    p.add_argument("--out", default=str(HERE / "results.jsonl"))
    p.add_argument("--budget", type=int, default=780)
    args = p.parse_args()
    if args.cmd == "run":
        run(args)
    elif args.cmd == "run2":
        run2(args)
        return
    elif args.cmd == "noise":
        print("| clip | " + " | ".join(f"{p} {c}" for p, c in NOISE_COLS) + " |")
        print("|" + "---|" * (len(NOISE_COLS) + 1))
        got = {(r["forms"][0], r["path"], r["cond"]): r["heard"]
               for r in map(json.loads, open(args.out2)) if r["kind"] == "noise"}
        for name in NOISES:
            cells = []
            for p_, c in NOISE_COLS:
                h = got.get((name, p_, c), "?")
                cells.append("(empty)" if h == "" else (h[:24] + "..." if len(h) > 24 else h))
            print(f"| {name} | " + " | ".join(cells) + " |")
        return
    elif args.cmd == "contexts":
        for k, v in contexts(load("commands.txt")).items():
            print(f"{k} ({len(v)}): {v[:200]}")
        return
    table(args)


if __name__ == "__main__":
    main()
