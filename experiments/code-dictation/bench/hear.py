"""Transcribe one benchmark clip as the plugin's push-to-talk path does
(dictate.py): the buffer's names as context, retried without context when
the model hears nothing. Usage: hear.py WAV < scope.
Prints {"text", "ms", "cached"}. Transcripts are cached per (clip, context)
in $EARS_BENCH_CACHE/asr.jsonl (run.py --asr-cache moves it), with the latency measured when first heard;
EARS_BENCH_FRESH=1 ignores the cache."""
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from probe import asr  # noqa: E402

wav = sys.argv[1]
context = sys.stdin.read().strip() or None
cache = os.path.join(os.environ["EARS_BENCH_CACHE"], "asr.jsonl")
key = hashlib.sha1(open(wav, "rb").read() + (context or "").encode()).hexdigest()

if os.environ.get("EARS_BENCH_FRESH") != "1" and os.path.exists(cache):
    for line in open(cache):
        r = json.loads(line)
        if r["key"] == key:
            print(json.dumps({"text": r["text"], "ms": r["ms"], "cached": True}))
            sys.exit(0)

text, seconds = asr(wav, system=context)
if text.startswith("RAW:"):  # context made it hear nothing; try without
    text, more = asr(wav)
    seconds += more
text = "" if text.startswith("RAW:") else text
ms = round(seconds * 1000)
with open(cache, "a") as f:
    f.write(json.dumps({"key": key, "text": text, "ms": ms}) + "\n")
print(json.dumps({"text": text, "ms": ms, "cached": False}))
