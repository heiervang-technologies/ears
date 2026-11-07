#!/usr/bin/env python3
"""Replay PCM clips in real time against an ears stream endpoint and time it.

usage: bench_stream.py WS_URL CLIP.pcm [CLIP.pcm ...] [--runs N] [--step MS] [--label NAME]

Per clip and run: every partial's server decode_ms and its lag (wall clock
audio sent minus audio_ms covered, on arrival), and the final's round trip
after `end` (what a user waits for after the silence wait). JSON lines on
stdout, summary on stderr.
"""
import argparse, asyncio, json, statistics, sys, time

import websockets

RATE = 16000
CHUNK_MS = 20


def pct(xs, q):
    if not xs:
        return None
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(q * len(xs)))], 1)


async def one(ws, uid, pcm, step):
    await ws.send(json.dumps({"type": "start", "utterance": uid, "language": "en",
                              "rollback_words": 3, "min_step_ms": step}))
    t0 = time.monotonic()
    partials, final = [], {}
    sent_ms = 0
    done = asyncio.Event()

    async def reader():
        nonlocal final
        async for raw in ws:
            m = json.loads(raw)
            if m.get("utterance") != uid:
                continue
            now = (time.monotonic() - t0) * 1000
            if m["type"] == "partial":
                partials.append({"at": now, "audio_ms": m.get("audio_ms"),
                                 "decode_ms": m.get("decode_ms"),
                                 "lag": now - m.get("audio_ms", 0)})
            elif m["type"] == "final":
                final = {"at": now, "decode_ms": m.get("decode_ms"), "text": m.get("text")}
                done.set()
                return
            elif m["type"] == "error":
                final = {"error": m}
                done.set()
                return

    task = asyncio.create_task(reader())
    step_bytes = RATE * 2 * CHUNK_MS // 1000
    for i in range(0, len(pcm), step_bytes):
        await ws.send(pcm[i:i + step_bytes])
        sent_ms += CHUNK_MS
        target = t0 + sent_ms / 1000
        delay = target - time.monotonic()
        if delay > 0:
            await asyncio.sleep(delay)
    end_at = (time.monotonic() - t0) * 1000
    await ws.send(json.dumps({"type": "end", "utterance": uid}))
    await asyncio.wait_for(done.wait(), 30)
    task.cancel()
    return {"audio_ms": len(pcm) // 32, "partials": len(partials),
            "partial_lag_p50": pct([p["lag"] for p in partials], .5),
            "partial_lag_p90": pct([p["lag"] for p in partials], .9),
            "decode_p50": pct([p["decode_ms"] for p in partials if p["decode_ms"]], .5),
            "decode_max": pct([p["decode_ms"] for p in partials if p["decode_ms"]], 1),
            "final_rt": round(final.get("at", float("nan")) - end_at, 1),
            "final_decode": final.get("decode_ms"), "text": final.get("text"),
            "error": final.get("error")}


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("clips", nargs="+")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--step", type=int, default=150)
    ap.add_argument("--label", default="")
    a = ap.parse_args()
    async with websockets.connect(a.url, max_size=None) as ws:
        ready = json.loads(await ws.recv())
        print(json.dumps({"ready": ready}), file=sys.stderr)
        uid = int(time.time())
        for clip in a.clips:
            pcm = open(clip, "rb").read()
            rows = []
            for r in range(a.runs):
                uid += 1
                row = await one(ws, uid, pcm, a.step)
                row.update(clip=clip.rsplit("/", 1)[-1], run=r, step=a.step, label=a.label)
                print(json.dumps(row), flush=True)
                rows.append(row)
                await asyncio.sleep(0.5)
            f = [x["final_rt"] for x in rows]
            print(f"{a.label:10} {rows[0]['clip']:10} step={a.step:3} final_rt p50={statistics.median(f):.0f} ms "
                  f"partial_lag p50={statistics.median([x['partial_lag_p50'] or 0 for x in rows]):.0f} "
                  f"decode p50={statistics.median([x['decode_p50'] or 0 for x in rows]):.0f} "
                  f"partials={rows[0]['partials']}", file=sys.stderr)


asyncio.run(main())
