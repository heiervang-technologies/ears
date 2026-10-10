#!/usr/bin/env python3
"""Profile continuous live decoding ticks against a running Qwen3-ASR vLLM server.

Replays a 16 kHz mono WAV through the same per-tick HTTP requests that
`src/continuous.rs` sends (8 s audio windows in one block, settled text forced
as the assistant prefix, rollback words left open) and splits each tick's
latency using vLLM's Prometheus histograms:

  queue    waiting in the scheduler
  prefill  scheduled -> first token (audio encoder + LLM prefill)
  decode   first token -> finished (one forward pass per generated token)
  outside  client wall time minus queue/prefill/decode (HTTP, JSON, base64,
           audio loading, mel features, tokenization, detokenization)

The metrics are server-wide, so a tick only counts when exactly one request
finished while it ran; ticks overlapped by other clients are reported but
excluded from the summary.

  python profile_tick.py clip.wav --url http://localhost:30189 --out ticks.jsonl

Standard library only.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import statistics
import struct
import sys
import time
import urllib.error
import urllib.request
import wave

SAMPLE_RATE = 16_000
ENCODER_WINDOW = 8 * SAMPLE_RATE
MIN_ITEM = SAMPLE_RATE // 10
ASR_TAG = "<asr_text>"

# Same template as CHAT_TEMPLATE in src/continuous.rs.
CHAT_TEMPLATE = "".join([
    "{%- set ns = namespace(n=0, asst='', sys='') -%}",
    "{%- for m in messages -%}",
    "{%- if m.role == 'system' -%}",
    "{%- set ns.sys = m.content if m.content is string else (m.content | map(attribute='text') | join('')) -%}",
    "{%- endif -%}",
    "{%- if m.role == 'assistant' -%}",
    "{%- set ns.asst = m.content if m.content is string else (m.content | map(attribute='text') | join('')) -%}",
    "{%- endif -%}",
    "{%- if m.role == 'user' and m.content is not string -%}",
    "{%- for c in m.content -%}",
    "{%- if c.type == 'audio' or c.type == 'input_audio' or ('audio' in c) or ('audio_url' in c) -%}",
    "{%- set ns.n = ns.n + 1 -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
    r"{{- '<|im_start|>system\n' + ns.sys + '<|im_end|>\n<|im_start|>user\n<|audio_start|>' -}}",
    "{{- '<|audio_pad|>' * ns.n -}}",
    r"{{- '<|audio_end|><|im_end|>\n<|im_start|>assistant\n' + ns.asst -}}",
])

HISTOGRAMS = {
    "queue": "vllm:request_queue_time_seconds",
    "prefill": "vllm:request_prefill_time_seconds",
    "decode": "vllm:request_decode_time_seconds",
    "inference": "vllm:request_inference_time_seconds",
    "e2e": "vllm:e2e_request_latency_seconds",
}
COUNTERS = {
    "prefix_queries": "vllm:prefix_cache_queries_total",
    "prefix_hits": "vllm:prefix_cache_hits_total",
    "mm_queries": "vllm:mm_cache_queries_total",
    "mm_hits": "vllm:mm_cache_hits_total",
}


def read_pcm(path: str) -> list[int]:
    with wave.open(path, "rb") as w:
        if w.getframerate() != SAMPLE_RATE or w.getnchannels() != 1 or w.getsampwidth() != 2:
            sys.exit(f"{path}: need 16 kHz mono PCM16")
        raw = w.readframes(w.getnframes())
    return list(struct.unpack(f"<{len(raw) // 2}h", raw))


def wav_b64(pcm: list[int]) -> str:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(struct.pack(f"<{len(pcm)}h", *pcm))
    return base64.b64encode(buf.getvalue()).decode()


def settled_prefix(text: str, words: int) -> str:
    """Port of `settled_prefix` in src/continuous.rs."""
    if words == 0:
        return text.rstrip()
    starts = [i for i, c in enumerate(text)
              if not c.isspace() and (i == 0 or text[i - 1].isspace())]
    if len(starts) <= words:
        return ""
    return text[:starts[-words]].rstrip()


def scrape(url: str) -> dict[str, float]:
    """Sum the wanted series over all label sets."""
    body = urllib.request.urlopen(f"{url}/metrics", timeout=5).read().decode()
    want = {f"{v}_sum": f"{k}_sum" for k, v in HISTOGRAMS.items()}
    want.update({f"{v}_count": f"{k}_count" for k, v in HISTOGRAMS.items()})
    want.update({v: k for k, v in COUNTERS.items()})
    out = dict.fromkeys(want.values(), 0.0)
    for line in body.splitlines():
        if line.startswith("#"):
            continue
        name = line.split("{", 1)[0].split(" ", 1)[0]
        if name in want:
            out[want[name]] += float(line.rsplit(" ", 1)[1])
    return out


class Decoder:
    """Mirror of ContinuousDecoder::step with the language already pinned."""

    def __init__(self, url, model, language, rollback, uuids=False):
        self.url = f"{url}/v1/chat/completions"
        self.model = model
        self.header = f"language {language}{ASR_TAG}"
        self.rollback = rollback
        self.stable = ""
        # With uuids, a closed window is uploaded once with a content-hash
        # uuid and afterwards referenced by that uuid alone (vLLM looks it up
        # in its multimodal processor cache).
        self.uuids = uuids
        self.sent: set[str] = set()

    def body(self, pcm, max_tokens=None):
        audio = []
        for i in range(0, len(pcm), ENCODER_WINDOW):
            w = pcm[i:i + ENCODER_WINDOW]
            if len(w) < MIN_ITEM:
                continue
            closed = len(w) == ENCODER_WINDOW
            if self.uuids and closed:
                data = wav_b64(w)
                uid = "ears-w-" + hashlib.sha256(data.encode()).hexdigest()[:32]
                if uid in self.sent:
                    data = ""
                audio.append({"type": "input_audio", "uuid": uid,
                              "input_audio": {"data": data, "format": "wav"}})
            else:
                audio.append({"type": "input_audio",
                              "input_audio": {"data": wav_b64(w), "format": "wav"}})
        return {
            "model": self.model,
            "messages": [{"role": "user", "content": audio},
                         {"role": "assistant", "content": self.header + self.stable}],
            "add_generation_prompt": False,
            "continue_final_message": True,
            "chat_template": CHAT_TEMPLATE,
            "temperature": 0.0,
            "max_tokens": max_tokens or 64 + 5 * len(pcm) // SAMPLE_RATE,
        }

    def post(self, body):
        data = json.dumps(body).encode()
        req = urllib.request.Request(self.url, data=data,
                                     headers={"Content-Type": "application/json"})
        t0 = time.perf_counter()
        try:
            reply = json.loads(urllib.request.urlopen(req, timeout=30).read())
        except urllib.error.HTTPError as e:
            sys.exit(f"HTTP {e.code}: {e.read().decode(errors='replace')[:2000]}")
        wall = (time.perf_counter() - t0) * 1000
        for part in body["messages"][0]["content"]:
            if "uuid" in part:
                self.sent.add(part["uuid"])
        return reply, wall, len(data)

    def settle(self, continuation, last):
        hyp = (self.stable + continuation).lstrip()
        if not last:
            settled = settled_prefix(hyp, self.rollback)
            if len(settled) > len(self.stable) and settled.startswith(self.stable):
                self.stable = settled
        return hyp.rstrip()


def measured(url, fn):
    before = scrape(url)
    result = fn()
    after = scrape(url)
    d = {k: after[k] - before[k] for k in before}
    sample = {k: d[f"{k}_sum"] * 1000 for k in HISTOGRAMS}
    sample["isolated"] = d["e2e_count"] == 1
    for k in COUNTERS:
        sample[k] = d[k]
    return result, sample


def summarize(rows, label):
    ok = [r for r in rows if r["isolated"]]
    print(f"\n{label}: {len(ok)} isolated ticks of {len(rows)}")
    if not ok:
        return
    cols = ["wall", "outside", "queue", "prefill", "decode", "completion_tokens",
            "prompt_tokens", "payload_kb", "ms_per_decode_token"]
    print(f"  {'':22}{'p50':>9}{'p90':>9}{'max':>9}")
    for c in cols:
        vals = sorted(r[c] for r in ok if r.get(c) is not None)
        if not vals:
            continue
        p = lambda q: vals[min(len(vals) - 1, int(q * len(vals)))]
        print(f"  {c:22}{p(0.5):9.1f}{p(0.9):9.1f}{vals[-1]:9.1f}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("wav")
    ap.add_argument("--url", default="http://localhost:30189")
    ap.add_argument("--language", default="English")
    ap.add_argument("--step-ms", type=int, default=150)
    ap.add_argument("--start-ms", type=int, default=1000)
    ap.add_argument("--rollback", type=int, default=3)
    ap.add_argument("--floor", action="store_true",
                    help="after each tick, resend it with max_tokens=1 (fully cached floor)")
    ap.add_argument("--uuids", action="store_true",
                    help="reference already-uploaded closed windows by uuid instead of resending them")
    ap.add_argument("--out", help="write one JSON line per tick")
    args = ap.parse_args()

    url = args.url.rstrip("/")
    model = json.loads(urllib.request.urlopen(f"{url}/v1/models", timeout=5).read())["data"][0]["id"]
    pcm = read_pcm(args.wav)
    dec = Decoder(url, model, args.language, args.rollback, uuids=args.uuids)
    step = args.step_ms * SAMPLE_RATE // 1000
    ends = list(range(args.start_ms * SAMPLE_RATE // 1000, len(pcm), step)) + [len(pcm)]

    rows, floors = [], []
    out = open(args.out, "w") if args.out else None
    for n, end in enumerate(ends):
        last = end == len(pcm)
        body = dec.body(pcm[:end])
        (reply, wall, size), s = measured(url, lambda: dec.post(body))
        choice = reply["choices"][0]
        hyp = dec.settle(choice["message"]["content"], last)
        usage = reply.get("usage", {})
        row = dict(s, audio_s=end / SAMPLE_RATE, windows=-(-end // ENCODER_WINDOW),
                   wall=wall, payload_kb=size / 1024,
                   prompt_tokens=usage.get("prompt_tokens"),
                   completion_tokens=usage.get("completion_tokens"),
                   finish=choice.get("finish_reason"), stable_chars=len(dec.stable), text=hyp)
        row["outside"] = wall - s["queue"] - s["prefill"] - s["decode"]
        ct = row["completion_tokens"] or 0
        row["ms_per_decode_token"] = s["decode"] / (ct - 1) if ct > 1 else None
        rows.append(row)
        if args.floor:
            fbody = dec.body(pcm[:end], max_tokens=1)
            (_, fwall, _), fs = measured(url, lambda: dec.post(fbody))
            frow = dict(fs, wall=fwall, audio_s=row["audio_s"])
            frow["outside"] = fwall - fs["queue"] - fs["prefill"] - fs["decode"]
            floors.append(frow)
            row["floor_wall"] = fwall
        if out:
            out.write(json.dumps(row) + "\n")
        print(f"{row['audio_s']:6.2f}s w{row['windows']} {wall:6.1f}ms "
              f"pre {s['prefill']:5.1f} dec {s['decode']:5.1f} out {row['outside']:5.1f} "
              f"gen {ct:3} {'' if s['isolated'] else '[overlap] '}| {hyp[-60:]}",
              flush=True)

    summarize(rows, "ticks")
    for w in sorted({r["windows"] for r in rows}):
        summarize([r for r in rows if r["windows"] == w], f"ticks with {w} window(s)")
    if floors:
        summarize(floors, "floor (same request again, max_tokens=1, everything cached)")
    print(f"\nfinal: {rows[-1]['text']}")


if __name__ == "__main__":
    main()
