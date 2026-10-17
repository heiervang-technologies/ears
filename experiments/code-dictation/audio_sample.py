"""A small audio sample of the dataset, and how the current ASR hears it.

Picks spoken lines from records.jsonl, synthesises each with two Kokoro
voices and espeak-ng, transcribes every clip with Qwen3-ASR twice (no
context, then the file's names as context, as dictate.py does), and scores:
  wer      word error of the transcript against the spoken text (both
           through spoken.plain: lowercase, no punctuation, numbers as digits)
  code     for terse and explicit lines: the transcript through spoken.py
           gives the original code (same AST; lenient: string case/spacing
           forgiven)

Requests are sequential. Usage:
  python audio_sample.py synth  [--lines 100]   # TTS -> DIR/audio/*.wav
  python audio_sample.py asr                    # -> DIR/asr.jsonl
  python audio_sample.py report
"""
import argparse
import collections
import json
import os
import random
import subprocess
import sys
import time
import urllib.request

import spoken
from probe import asr
from roundtrip import back, same

DIR = "/run/media/me/storage/builds/ears/code-dictation-verbalizer"
TTS = "http://localhost:30383/v1/audio/speech"
# Kokoro is used only for clips already made: the service (2 GiB limit) was
# OOM-killed every 10 to 15 requests, so new clips use two espeak-ng voices.
VOICES = ["kokoro:af_heart", "kokoro:bm_george", "espeak", "espeak:en-us+f3"]
NEW_KOKORO = False
QUOTA = {"terse": 40, "explicit": 30, "natural": 30}


def pick(n_lines, seed=0):
    """Reservoir-sample records per style: 3-25 words, and for terse and
    explicit only lines spoken.py takes back (so a miss is the ASR's)."""
    rng = random.Random(seed)
    quota = {s: round(q * n_lines / 100) for s, q in QUOTA.items()}
    pools, seen = collections.defaultdict(list), collections.Counter()
    for line in open(os.path.join(DIR, "dataset", "records.jsonl")):
        r = json.loads(line)
        s = r["style"]
        if s not in quota or not 3 <= len(r["spoken"].split()) <= 25 or r.get("verified") is False:
            continue
        seen[s] += 1
        if len(pools[s]) < quota[s]:
            pools[s].append(r)
        elif (k := rng.randrange(seen[s])) < quota[s]:
            pools[s][k] = r
    return [r for s in quota for r in pools[s]]


def synth(text, voice, path):
    raw = path + ".raw.wav"
    if voice == "espeak":
        subprocess.run(["espeak-ng", "-s", "150", "-w", raw, text], check=True)
    elif voice.startswith("espeak:"):
        subprocess.run(["espeak-ng", "-v", voice.split(":")[1], "-s", "135", "-w", raw, text], check=True)
    else:
        body = json.dumps({"input": text, "voice": voice.split(":")[1], "response_format": "wav"}).encode()
        for attempt in range(3):  # the service drops a connection now and then
            try:
                req = urllib.request.Request(TTS, body, {"Content-Type": "application/json"})
                data = urllib.request.urlopen(req, timeout=120).read()
                break
            except OSError as e:
                print(f"TTS failed ({e}), retrying", file=sys.stderr)
                time.sleep(15 * (attempt + 1))
        else:
            raise RuntimeError("TTS unavailable")
        open(raw, "wb").write(data)
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", raw, "-ar", "16000", "-ac", "1", path], check=True)
    os.remove(raw)


def wer(ref, hyp):
    r, h = spoken.plain(ref).split(), spoken.plain(hyp).split()
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
    return d[len(h)], len(r)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("step", choices=["synth", "asr", "report"])
    ap.add_argument("--lines", type=int, default=100)
    a = ap.parse_args()
    audio = os.path.join(DIR, "audio")
    manifest = os.path.join(DIR, "audio", "manifest.jsonl")
    if a.step == "synth":
        os.makedirs(audio, exist_ok=True)
        with open(manifest, "w") as out:
            for i, r in enumerate(pick(a.lines)):
                for voice in VOICES:
                    path = os.path.join(audio, f"{i:03d}-{voice.split(':')[-1]}.wav")
                    if not os.path.exists(path):
                        if voice.startswith("kokoro") and not NEW_KOKORO:
                            continue
                        synth(r["spoken"], voice, path)
                    out.write(json.dumps({**r, "voice": voice, "wav": path}) + "\n")
                print(i, r["style"], r["spoken"], file=sys.stderr)
    elif a.step == "asr":
        scopes = {}
        for line in open(os.path.join(DIR, "dataset", "scopes.jsonl")):
            s = json.loads(line)
            scopes[s["path"]] = s["scope"]
        with open(os.path.join(DIR, "asr.jsonl"), "w") as out:
            for line in open(manifest):
                r = json.loads(line)
                scope = scopes.get(r["path"], [])
                for ctx in (False, True):
                    heard, dt = asr(r["wav"], system=" ".join(scope[:300]) if ctx else None)
                    row = {**r, "context": ctx, "heard": heard, "ms": round(dt * 1000)}
                    row["errors"], row["words"] = wer(r["spoken"], heard)
                    if r["style"] in ("terse", "explicit"):
                        got = back(heard, scope)
                        row["got"] = got
                        row["code_exact"] = same(r["code"], got)
                        row["code_lenient"] = same(r["code"], got, lenient=True)
                    out.write(json.dumps(row) + "\n")
                    print(f"{r['voice']:16} ctx={ctx:d} {r['spoken']!r}\n{'':21}-> {heard!r}", file=sys.stderr)
    else:
        report([json.loads(l) for l in open(os.path.join(DIR, "asr.jsonl"))])


def report(rows):
    def line(name, rs):
        e, w = sum(r["errors"] for r in rs), sum(r["words"] for r in rs)
        code = [r for r in rs if "code_exact" in r]
        exact = sum(r["code_exact"] for r in code)
        lenient = sum(r["code_lenient"] for r in code)
        perfect = sum(r["errors"] == 0 for r in rs)
        print(f"  {name:28} n={len(rs):3}  WER {100 * e / max(w, 1):5.1f}%  all words right {100 * perfect / len(rs):5.1f}%"
              + (f"  code exact {100 * exact / len(code):5.1f}%  lenient {100 * lenient / len(code):5.1f}%" if code else ""))
    for ctx in (False, True):
        print(f"context={'scope names' if ctx else 'none'}")
        rs = [r for r in rows if r["context"] == ctx]
        line("all", rs)
        for v in VOICES:
            line(v, [r for r in rs if r["voice"] == v])
        for s in QUOTA:
            line("style " + s, [r for r in rs if r["style"] == s])
    print(f"median latency {sorted(r['ms'] for r in rows)[len(rows) // 2]} ms")
    for ctx in (False, True):
        perfect = [r for r in rows if r["context"] == ctx and r["errors"] == 0 and "code_exact" in r]
        if perfect:
            print(f"word-perfect transcripts (context={ctx}): code exact "
                  f"{100 * sum(r['code_exact'] for r in perfect) / len(perfect):.1f}% of {len(perfect)}")
    punct = [r for r in rows if not r["context"] and any(c in r["heard"].rstrip(".") for c in ",_()=[]")]
    print(f"transcripts (no context) writing symbols the words named (',', '_', '(' ...): {len(punct)} of "
          f"{sum(not r['context'] for r in rows)}; with context: "
          f"{sum(r['context'] and any(c in r['heard'].rstrip('.') for c in ',_()=[]') for r in rows)}")
    # the most common word confusions (no context, aligned only when equal length)
    conf = collections.Counter()
    for r in rows:
        ref, hyp = spoken.plain(r["spoken"]).split(), spoken.plain(r["heard"]).split()
        if len(ref) == len(hyp):
            conf.update(f"{x}->{y}" for x, y in zip(ref, hyp) if x != y)
    print("common substitutions:", ", ".join(f"{k} ({n})" for k, n in conf.most_common(25)))


if __name__ == "__main__":
    main()
