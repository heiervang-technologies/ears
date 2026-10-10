#!/usr/bin/env python3
"""Check exact tail verification against plain continuous decoding, over HTTP.

Every tick runs two decoders on the same audio:

  baseline  today's decoder: force header + settled text, decode the rest.
  verify    force header + settled text + the previous tick's open words
            (the draft), read the model's own top-1 token at every draft
            position from prompt logprobs, and accept the longest run where
            the draft token *is* the top-1. Fully accepted: the generation
            that followed the draft is exactly greedy, so it is kept. Rejected:
            fall back to a baseline request (the plugin would instead
            continue from the accepted tokens plus the top-1 token).

Reports per tick whether both hypotheses are identical, how many draft
tokens were accepted, and how many tokens each path had to decode.

Prompt logprobs make vLLM skip the prefix cache for that request, so the
verify request's latency here is NOT what the plugin would see; the timing
probe ("accept") replays an accepted tick without prompt logprobs to show the
cache-friendly cost.

  python tail_verify_sim.py clip.wav --url http://localhost:30189
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request

from profile_tick import SAMPLE_RATE, Decoder, read_pcm


class Verifier(Decoder):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.draft = ""  # previous hypothesis minus the settled text

    def request(self, pcm, prefix_extra="", prompt_logprobs=None):
        body = self.body(pcm)
        body["messages"][-1]["content"] += prefix_extra
        if prompt_logprobs is not None:
            body["prompt_logprobs"] = prompt_logprobs
            body["return_token_ids"] = True
        return self.post(body)

    def remember(self, hyp_full):
        """hyp_full: stable + continuation, before trimming."""
        self.draft = hyp_full[len(self.stable):] if hyp_full.startswith(self.stable) else ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("wav")
    ap.add_argument("--url", default="http://localhost:30189")
    ap.add_argument("--step-ms", type=int, default=150)
    ap.add_argument("--start-ms", type=int, default=1000)
    ap.add_argument("--control", action="store_true",
                    help="never draft: run two plain decoders to measure the noise floor")
    args = ap.parse_args()

    url = args.url.rstrip("/")
    model = json.loads(urllib.request.urlopen(f"{url}/v1/models", timeout=5).read())["data"][0]["id"]
    pcm = read_pcm(args.wav)
    base = Decoder(url, model, "English", 3)
    ver = Verifier(url, model, "English", 3)
    step = args.step_ms * SAMPLE_RATE // 1000
    ends = list(range(args.start_ms * SAMPLE_RATE // 1000, len(pcm), step)) + [len(pcm)]

    stats = dict(ticks=0, same=0, full_accept=0, rejected=0, no_draft=0,
                 base_tokens=0, ver_tokens=0, accepted_tokens=0, draft_tokens=0)
    accept_ms, base_ms = [], []
    for end in ends:
        last = end == len(pcm)
        clip = pcm[:end]

        # Baseline tick.
        breply, bwall, _ = base.post(base.body(clip))
        bcont = breply["choices"][0]["message"]["content"]
        bfull = (base.stable + bcont).lstrip()
        bhyp = base.settle(bcont, last)
        stats["base_tokens"] += breply["usage"]["completion_tokens"]
        base_ms.append(bwall)

        # Verify tick.
        draft = "" if args.control else ver.draft
        if not draft.strip():
            stats["no_draft"] += 1
            reply, _, _ = ver.request(clip)
            cont = reply["choices"][0]["message"]["content"]
            stats["ver_tokens"] += reply["usage"]["completion_tokens"]
        else:
            reply, _, _ = ver.request(clip, draft, prompt_logprobs=1)
            ids = reply["prompt_token_ids"]
            plp = reply["prompt_logprobs"]
            # Draft tokens are the trailing prompt tokens whose text sums to the draft.
            need, got, i = len(draft.encode()), 0, len(ids)
            while got < need:
                i -= 1
                got += len(plp[i][str(ids[i])]["decoded_token"].encode())
            n_draft = len(ids) - i
            accepted = 0
            for p in range(i, len(ids)):
                if plp[p][str(ids[p])]["rank"] != 1:
                    break
                accepted += 1
            stats["draft_tokens"] += n_draft
            stats["accepted_tokens"] += accepted
            if accepted == n_draft:
                stats["full_accept"] += 1
                cont = draft + reply["choices"][0]["message"]["content"]
                stats["ver_tokens"] += reply["usage"]["completion_tokens"]
                # Cache-friendly timing of the same accepted tick.
                _, awall, _ = ver.request(clip, draft)
                accept_ms.append(awall)
            else:
                stats["rejected"] += 1
                reply, _, _ = ver.request(clip)
                cont = reply["choices"][0]["message"]["content"]
                stats["ver_tokens"] += reply["usage"]["completion_tokens"]
        vfull = (ver.stable + cont).lstrip()
        vhyp = ver.settle(cont, last)
        ver.remember(vfull)

        stats["ticks"] += 1
        same = vhyp == bhyp
        stats["same"] += same
        if not same:
            print(f"DIFF @{end / SAMPLE_RATE:.2f}s\n  base: {bhyp[-90:]}\n  ver:  {vhyp[-90:]}",
                  flush=True)
        if base.stable != ver.stable:
            print(f"stable diverged @{end / SAMPLE_RATE:.2f}s; resyncing verifier", flush=True)
            ver.stable = base.stable
            ver.remember(bfull)

    print(json.dumps(stats, indent=1))
    t = stats["ticks"]
    print(f"identical hypotheses: {stats['same']}/{t}")
    print(f"decode tokens per tick: baseline {stats['base_tokens'] / t:.2f}, "
          f"verify {stats['ver_tokens'] / t:.2f}")
    if stats["draft_tokens"]:
        print(f"draft acceptance: {stats['accepted_tokens']}/{stats['draft_tokens']} tokens, "
              f"{stats['full_accept']} full / {stats['rejected']} rejected ticks")
    if accept_ms:
        accept_ms.sort()
        base_ms.sort()
        print(f"HTTP wall p50: baseline {base_ms[len(base_ms) // 2]:.1f} ms, "
              f"accepted tick replay {accept_ms[len(accept_ms) // 2]:.1f} ms")
    print(f"final baseline: {bhyp}\nfinal verify:   {vhyp}")


if __name__ == "__main__":
    sys.exit(main())
