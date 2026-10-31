"""Check the incremental engine against batch decoding and time it.

    python bench.py MODEL_DIR clip.wav [clip.wav ...] [--step-ms 300] [--threads N]

For each clip: batch transcript and time; then a simulated live session
feeding `--step-ms` of audio per update (settled tokens forced, the rest as a
draft), reporting per-update cost and whether the final matches batch.
"""

import argparse
import re
import time
import wave

import numpy as np

import engine


def load(path):
    with wave.open(path) as w:
        sr, ch = w.getframerate(), w.getnchannels()
        a = np.frombuffer(w.readframes(w.getnframes()), "<i2").astype(np.float32) / 32768
    if ch > 1:
        a = a.reshape(-1, ch).mean(1)
    if sr != engine.SAMPLE_RATE:
        n = int(len(a) * engine.SAMPLE_RATE / sr)
        a = np.interp(np.linspace(0, len(a) - 1, n), np.arange(len(a)), a).astype(np.float32)
    return a


def words(text):
    return re.sub(r"[^\w\s']", " ", text.lower()).split()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("model")
    ap.add_argument("clips", nargs="+")
    ap.add_argument("--step-ms", type=int, default=300)
    ap.add_argument("--rollback", type=int, default=3)
    ap.add_argument("--settle-s", type=float, default=1.0)
    ap.add_argument("--threads", type=int)
    args = ap.parse_args()
    m = engine.Model(args.model, args.threads)
    m.transcribe(np.zeros(engine.SAMPLE_RATE, np.float32))  # warm up

    for path in args.clips:
        audio = load(path)
        secs = len(audio) / engine.SAMPLE_RATE
        t = time.monotonic()
        batch = m.transcribe(audio)
        tb = time.monotonic() - t
        print(f"\n== {path} ({secs:.1f} s)")
        print(f"batch  {tb * 1000:7.0f} ms  RTF {tb / secs:.3f}  {batch!r}")

        live = engine.Live(m, args.rollback, args.settle_s)
        step = engine.SAMPLE_RATE * args.step_ms // 1000
        costs, steps, accepted, stable_lag = [], [], [], []
        for i in range(0, len(audio), step):
            t = time.monotonic()
            live.update(audio[i:i + step])
            costs.append(time.monotonic() - t)
            steps.append(live.stream.last_steps)
            accepted.append(live.stream.last_draft_accepted)
            stable_lag.append(len(live.tokens) - live.stable)
        t = time.monotonic()
        live.update(final=True)
        tf = time.monotonic() - t
        text = live.text()
        c = np.array(costs) * 1000
        print(f"live   {len(c)} updates of {args.step_ms} ms: mean {c.mean():.0f} ms, p95 "
              f"{np.percentile(c, 95):.0f} ms, max {c.max():.0f} ms, last {c[-1]:.0f} ms; "
              f"final {tf * 1000:.0f} ms")
        print(f"       AR steps/update mean {np.mean(steps):.1f}, draft tokens accepted mean "
              f"{np.mean(accepted):.1f}; unsettled tokens mean {np.mean(stable_lag):.1f}; "
              f"conflicts {live.conflicts}; busy {sum(costs) / secs:.0%} of real time")
        same = "== batch" if text == batch else "words == batch" if words(text) == words(batch) else "!= batch"
        print(f"final  {same}  {text!r}")


if __name__ == "__main__":
    main()
