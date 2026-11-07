#!/usr/bin/env python3
"""Fixed evaluation for the ears ASR autoresearch loop. Do not edit during a run.

Scores one server (Qwen3-ASR on vLLM with the ears stream plugin) on:

  latency  single stream, real-time replay of the latency set (16 kHz PCM16)
           - final_rt: ms from `end` to `final` (what the user waits for after
             the silence wait), p50 and p90
           - decode:   server `decode_ms` of partials, p50 and p90
  wer      the stream path as ears uses it (start, audio, end, final), paced
           at --pace x real time with --concurrency streams at once, over the
           WER sets; per set and pooled (word-weighted)

usage:
  eval.py --url http://127.0.0.1:8000 --manifest data/manifest.jsonl \
          --out results/<exp>.json [--pace 3] [--concurrency 4] [--quick]

Manifest lines: {"id", "pcm" (path, relative to the manifest), "text", "set",
"latency": bool}. Sets: "ls-clean", "ls-other", "cmd", "code", ...
Prints one summary line; writes the full result JSON to --out.
"""
import argparse, asyncio, json, os, re, statistics, sys, time, unicodedata

import websockets

RATE = 16000
CHUNK_MS = 20
BYTES_PER_MS = RATE * 2 // 1000


# ---------- text normalization and WER ----------

def normalize(text: str) -> list[str]:
    t = unicodedata.normalize("NFKC", text or "").lower()
    t = t.replace("’", "'")
    t = re.sub(r"[^\w\s']", " ", t)       # punctuation out, keep apostrophes
    t = re.sub(r"(?<!\w)'|'(?!\w)", " ", t)  # quotes, not contractions
    t = t.replace("_", " ")
    return t.split()


def edits(ref: list[str], hyp: list[str]) -> int:
    prev = list(range(len(hyp) + 1))
    for i, r in enumerate(ref, 1):
        cur = [i] + [0] * len(hyp)
        for j, h in enumerate(hyp, 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (r != h))
        prev = cur
    return prev[-1]


# ---------- one stream utterance ----------

class Conn:
    """One websocket, one utterance at a time."""

    def __init__(self, url: str):
        self.url = url.rstrip("/").replace("http", "ws", 1) + "/v1/ears/stream"
        self.ws = None
        self.uid = int(time.time() * 1000) % 10**9

    async def open(self):
        self.ws = await websockets.connect(self.url, max_size=None, open_timeout=30)
        ready = json.loads(await self.ws.recv())
        if ready.get("type") != "ready":
            raise RuntimeError(f"no ready: {ready}")
        return ready

    async def run(self, pcm: bytes, pace: float, step_ms: int | None = None):
        self.uid += 1
        uid = self.uid
        start = {"type": "start", "utterance": uid, "language": "en"}
        if step_ms is not None:
            start["min_step_ms"] = step_ms
        await self.ws.send(json.dumps(start))
        t0 = time.monotonic()
        partials, final = [], {}
        done = asyncio.Event()

        async def reader():
            nonlocal final
            async for raw in self.ws:
                if isinstance(raw, bytes):
                    continue
                m = json.loads(raw)
                if m.get("utterance") != uid:
                    continue
                now = (time.monotonic() - t0) * 1000
                if m["type"] == "partial":
                    partials.append({"at": now, "audio_ms": m.get("audio_ms"),
                                     "decode_ms": m.get("decode_ms")})
                elif m["type"] in ("final", "error"):
                    final = dict(m, at=now)
                    done.set()
                    return

        task = asyncio.create_task(reader())
        step = CHUNK_MS * BYTES_PER_MS
        sent_ms = 0
        for i in range(0, len(pcm), step):
            await self.ws.send(pcm[i:i + step])
            sent_ms += CHUNK_MS
            delay = t0 + sent_ms / 1000 / pace - time.monotonic()
            if delay > 0:
                await asyncio.sleep(delay)
        end_at = (time.monotonic() - t0) * 1000
        await self.ws.send(json.dumps({"type": "end", "utterance": uid}))
        try:
            await asyncio.wait_for(done.wait(), 60)
        finally:
            task.cancel()
        return {"text": final.get("text"), "error": final.get("code") if final.get("type") == "error" else None,
                "final_rt": final.get("at", float("nan")) - end_at,
                "final_decode": final.get("decode_ms"),
                "decodes": [p["decode_ms"] for p in partials if p["decode_ms"] is not None]}


def pct(xs, q):
    xs = sorted(x for x in xs if x == x)
    if not xs:
        return None
    return round(xs[min(len(xs) - 1, int(q * len(xs)))], 1)


async def latency(url, items, runs):
    conn = Conn(url)
    await conn.open()
    rts, decs = [], []
    # one warm-up utterance, not scored
    await conn.run(items[0]["audio"], 1.0)
    for _ in range(runs):
        for it in items:
            r = await conn.run(it["audio"], 1.0)
            rts.append(r["final_rt"])
            decs.extend(r["decodes"])
            await asyncio.sleep(0.3)
    await conn.ws.close()
    return {"final_rt_p50": pct(rts, .5), "final_rt_p90": pct(rts, .9),
            "decode_p50": pct(decs, .5), "decode_p90": pct(decs, .9),
            "n": len(rts)}


async def wer(url, items, pace, concurrency):
    queue = asyncio.Queue()
    for it in items:
        queue.put_nowait(it)
    rows = []

    async def worker():
        conn = Conn(url)
        await conn.open()
        while True:
            try:
                it = queue.get_nowait()
            except asyncio.QueueEmpty:
                break
            try:
                r = await conn.run(it["audio"], pace)
            except Exception as e:  # reconnect once and record a failure
                r = {"text": None, "error": repr(e)}
                await conn.open()
            ref, hyp = normalize(it["text"]), normalize(r.get("text") or "")
            rows.append({"id": it["id"], "set": it["set"], "ref_words": len(ref),
                         "edits": edits(ref, hyp), "hyp": r.get("text"), "error": r.get("error")})
        await conn.ws.close()

    await asyncio.gather(*(worker() for _ in range(concurrency)))
    by = {}
    for r in rows:
        s = by.setdefault(r["set"], [0, 0, 0])
        s[0] += r["edits"]; s[1] += r["ref_words"]; s[2] += bool(r["error"])
    sets = {k: {"wer": round(100 * e / max(w, 1), 2), "words": w, "errors": n} for k, (e, w, n) in by.items()}
    e = sum(v[0] for v in by.values()); w = sum(v[1] for v in by.values())
    return {"pooled": round(100 * e / max(w, 1), 2), "sets": sets}, rows


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--pace", type=float, default=3.0)
    ap.add_argument("--concurrency", type=int, default=4)
    ap.add_argument("--latency-runs", type=int, default=2)
    ap.add_argument("--quick", action="store_true", help="1/4 of each WER set, for a smoke test")
    ap.add_argument("--skip-wer", action="store_true")
    ap.add_argument("--skip-latency", action="store_true")
    a = ap.parse_args()

    base = os.path.dirname(os.path.abspath(a.manifest))
    items = []
    for line in open(a.manifest):
        it = json.loads(line)
        it["audio"] = open(os.path.join(base, it["pcm"]), "rb").read()
        items.append(it)
    lat_items = [it for it in items if it.get("latency")]
    wer_items = [it for it in items if not it.get("latency")]
    if a.quick:
        wer_items = wer_items[::4]

    t = time.time()
    out = {"url": a.url, "started": t}
    if not a.skip_latency:
        out["latency"] = await latency(a.url, lat_items, a.latency_runs)
    if not a.skip_wer:
        out["wer"], out["rows"] = await wer(a.url, wer_items, a.pace, a.concurrency)
    out["seconds"] = round(time.time() - t, 1)
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    json.dump(out, open(a.out, "w"), indent=1)
    lat, w = out.get("latency", {}), out.get("wer", {})
    print(f"final_rt_p50={lat.get('final_rt_p50')} final_rt_p90={lat.get('final_rt_p90')} "
          f"decode_p50={lat.get('decode_p50')} wer={w.get('pooled')} "
          + " ".join(f"{k}={v['wer']}" for k, v in sorted(w.get("sets", {}).items()))
          + f" secs={out['seconds']}")


if __name__ == "__main__":
    asyncio.run(main())
