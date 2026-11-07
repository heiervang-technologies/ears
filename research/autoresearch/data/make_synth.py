#!/usr/bin/env python3
"""Synthesize the command and code-dictation sets with an OpenAI-shaped TTS.

usage: make_synth.py OUT_DIR [--tts http://localhost:30384] [--actions ~/.config/ears/actions.toml]

Writes OUT_DIR/synth/<set>/<id>.pcm (16 kHz mono PCM16) and appends to
OUT_DIR/manifest.jsonl. Sets:
  cmd   command phrases (ears built-ins + a sample of actions.toml), 1 voice each
  code  code/agent dictation sentences (code_sentences.txt), 2 voices each
  latency  12 fixed clips for the real-time latency run (flag "latency")
No real person's voice: the voice list below excludes cloned voices of Markus.
"""
import argparse, json, os, random, re, subprocess, sys, tomllib, urllib.request
from concurrent.futures import ThreadPoolExecutor

VOICES = ["alice", "capri-en", "cleo", "hai", "janet", "leo", "maya", "mei", "mike", "ray"]
BUILTIN = ["over", "over and out", "scratch that", "new line", "enter", "clear", "clear all",
           "ears on", "ears off", "command", "dictation", "standby", "again", "undo", "wrong",
           "next", "previous", "back three", "literal over", "background"]
HERE = os.path.dirname(os.path.abspath(__file__))


def tts(url, text, voice, speed=1.0):
    body = json.dumps({"input": text, "voice": voice, "response_format": "wav", "speed": speed}).encode()
    req = urllib.request.Request(url.rstrip("/") + "/v1/audio/speech", body,
                                 {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        wav = r.read()
    out = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", "pipe:0", "-ac", "1", "-ar", "16000",
                          "-f", "s16le", "pipe:1"], input=wav, capture_output=True, check=True).stdout
    # 250 ms of leading and trailing silence, as VAD segments carry
    pad = b"\0\0" * 4000
    return pad + out + pad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--tts", default="http://localhost:30384")
    ap.add_argument("--actions", default=os.path.expanduser("~/.config/ears/actions.toml"))
    ap.add_argument("--n-actions", type=int, default=140)
    a = ap.parse_args()
    rng = random.Random(7)

    phrases = []
    try:
        acts = tomllib.load(open(a.actions, "rb")).get("action", [])
        for act in acts:
            for t in act.get("text", []):
                if re.fullmatch(r"[a-z0-9 ']+", t) and len(t.split()) <= 6:
                    phrases.append(t)
    except FileNotFoundError:
        pass
    phrases = sorted(set(phrases))
    rng.shuffle(phrases)
    cmd = BUILTIN + phrases[:a.n_actions]
    code = [l.strip() for l in open(os.path.join(HERE, "code_sentences.txt")) if l.strip()]

    jobs = []  # (set, id, text, voice, speed, latency)
    for i, t in enumerate(cmd):
        jobs.append(("cmd", f"cmd{i:03d}", t, VOICES[i % len(VOICES)], 1.0, False))
    for i, t in enumerate(code):
        for k in range(2):
            v = VOICES[(i * 2 + k) % len(VOICES)]
            jobs.append(("code", f"code{i:03d}{'ab'[k]}", t, v, rng.choice([0.9, 1.0, 1.15]), False))
    lat = ["Next track please.", "Over.", "Scratch that.", "Workspace three.",
           code[0], code[5], code[12], code[25], code[33], code[47],
           " ".join(code[40:44]), " ".join(code[50:55])]
    for i, t in enumerate(lat):
        jobs.append(("latency", f"lat{i:02d}", t, VOICES[i % len(VOICES)], 1.0, True))

    def run(job):
        s, id_, text, voice, speed, is_lat = job
        path = os.path.join("synth", s, id_ + ".pcm")
        full = os.path.join(a.out, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        if not os.path.exists(full):
            open(full, "wb").write(tts(a.tts, text, voice, speed))
        return {"id": id_, "pcm": path, "text": text, "set": s, "voice": voice,
                "speed": speed, "latency": is_lat}

    with ThreadPoolExecutor(2) as ex:  # modest: this TTS also serves `say`
        rows = list(ex.map(run, jobs))
    with open(os.path.join(a.out, "manifest.jsonl"), "a") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    print(f"{len(rows)} clips: " + ", ".join(f"{s}={sum(r['set'] == s for r in rows)}"
                                            for s in ("cmd", "code", "latency")))


if __name__ == "__main__":
    main()
