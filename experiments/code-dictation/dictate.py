"""Transcribe a WAV with names in scope as context, then translate to code.

Usage: python dictate.py [--nbest] LANG WAV [SCOPE_FILE]
       python dictate.py --text LANG TEXT [SCOPE_FILE]
       python dictate.py --serve   (JSON lines in and out, for the VAD path)
LANG is py, sh or text. Prints JSON: {"heard": ..., "code": ..., "alternatives":
[{"code": ..., "heard": ...}]}. With --nbest, alternatives come from the
model's beam-search hypotheses, best first, plus the words as heard.
"""
import json
import os
import struct
import sys
import tempfile

from probe import asr, asr_nbest
from spoken import bash_line, code_lines, insert_keys, normal_keys, python_line

def repaired(path):
    """pw-record fills in the WAV sizes only on exit; rebuild the header so a
    recording still in progress can be sent as a partial."""
    raw = open(path, "rb").read()
    at = raw.find(b"data")
    pcm = raw[at + 8:] if at >= 0 else b""
    pcm = pcm[: len(pcm) // 2 * 2]
    # One file per run: a partial and the final can read the same recording
    # at once, and must not delete each other's copy.
    fd, out = tempfile.mkstemp(suffix=".wav", dir=os.path.dirname(path))
    os.close(fd)
    header = b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt " + struct.pack(
        "<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16) + b"data" + struct.pack("<I", len(pcm))
    open(out, "wb").write(header + pcm)
    return out, len(pcm) / 32000


if sys.argv[1] == "--serve":  # translate JSON lines: {"id", "lang", "text", "scope", "mode"}
    for request in sys.stdin:
        r = json.loads(request)
        if r.get("mode") == "normal":
            keys = normal_keys(r["text"], r.get("scope", []), r["lang"])
            print(json.dumps({"id": r["id"], "keys": keys}), flush=True)
            continue
        keys = insert_keys(r["text"], r.get("scope", []), r["lang"])
        if keys:
            print(json.dumps({"id": r["id"], "keys": keys}), flush=True)
            continue
        lines = code_lines(r["lang"], r["text"], r.get("scope", []))
        print(json.dumps({"id": r["id"], "lines": lines}), flush=True)
    sys.exit(0)

if sys.argv[1] == "--text":  # translate only: python dictate.py --text LANG TEXT [SCOPE]
    scope = open(sys.argv[4]).read().split() if len(sys.argv) > 4 else []
    print(json.dumps({"lines": code_lines(sys.argv[2], sys.argv[3], scope)}))
    sys.exit(0)

nbest = "--nbest" in sys.argv
args = [a for a in sys.argv[1:] if a != "--nbest"]
lang, wav = args[0], args[1]
wav, seconds = repaired(wav)
if seconds < 0.3:
    os.remove(wav)
    print(json.dumps({"heard": "", "code": ""}))
    sys.exit(0)
scope = open(args[2]).read().split() if len(args) > 2 else []

context = " ".join(scope[:300]) or None
if nbest:
    hypotheses = [h for h in asr_nbest(wav, context) if h] or [h for h in asr_nbest(wav) if h]
else:
    heard, _ = asr(wav, system=context)
    if heard.startswith("RAW:"):  # context made it hear nothing; try without
        heard, _ = asr(wav)
    hypotheses = [] if heard.startswith("RAW:") else [heard]

translate = {"py": python_line, "sh": bash_line}.get(lang)
alternatives, seen = [], set()
for heard in hypotheses:
    code = translate(heard, scope) if translate else heard
    if code and code not in seen:
        seen.add(code)
        alternatives.append({"code": code, "heard": heard})
if nbest and translate and hypotheses:
    words = hypotheses[0].rstrip(".")  # escape hatch: the words themselves
    if words not in seen:
        alternatives.append({"code": words, "heard": "as heard"})
best = alternatives[0] if alternatives else {"code": "", "heard": ""}
lines = code_lines(lang, hypotheses[0], scope) if hypotheses else []
print(json.dumps({**best, "lines": lines, "alternatives": alternatives}))
os.remove(wav)
