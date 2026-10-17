"""Rank candidate code lines with the ASR's own decoder: no extra model.

    python rank_probe.py [scratch_dir]

For each clip, every candidate is forced as the assistant turn
("language English<asr_text>" + candidate) and scored by the summed
log-probability of its tokens (vLLM `prompt_logprobs`, one prefill each, no
generation). Score = log P(candidate | audio, preceding code), so the ranker
hears the audio as well as reading the code. Clips come from zero_shot_probe.py.
"""

import json, os, sys, time

import zero_shot_probe as z

SCRATCH = sys.argv[1] if len(sys.argv) > 1 else "/tmp/zsp"
PREFIX = "language English<asr_text>"

# case index -> candidates, the correct one first. The wrong ones are the
# kind of thing a rule translator emits when the parse is ambiguous.
CANDIDATES = {
    0: ["for i in range(10):", "for i in range(ten):", "for i in range(10)", "for iron range(10):"],
    1: ['print("hello", x)', 'print("hello, x")', "print(string, hello, x)"],
    2: ["def parse_args(argv):", "def pass_args(args):", "def parse_args(args):", "def parse(args, argv):"],
    3: ["if x is None:", "if x is non:", "if x is not None:"],
    4: ["return self.items[key]", "return self.items[index_key]", "return self.items.index(key)"],
    6: ["total = sum(sizes)", "total = sum(size)", "total == sum(sizes)"],
    8: ["x = len(names)", "x = len(name)", "x == len(names)"],
    10: ["args = parse_args(argv)", "args = pass_args(args)", "args = parse_args(args)"],
    11: ["result.append(item)", "result.depend(item)", "result.append(items)"],
    12: ["with open(path) as f:", "with open(path, f):", "with open(path as f):"],
    14: ["for key, value in data.items():", "for key, value in data.items:", "for key in value(data.items()):"],
    15: ['if __name__ == "__main__":', 'if name == "main":', "if name == main:"],
    16: ['raise ValueError("bad input")', "raise ValueError(bad_input)", 'raise ValueError("bad", input)'],
    17: ["count += 1", "count + 1", "count = 1"],
    18: ["lines = f.read().splitlines()", "lines = f.read().close().splitlines()", "lines = f.read(close).splitlines()"],
    19: ['print(f"total: {total}")', 'print(f"total: total")', 'print("total: {total}")'],
}


def score(path, candidate, context):
    messages = [{"role": "system", "content": context}, z.audio_msg(path),
                {"role": "assistant", "content": PREFIX + candidate}]
    body = {"model": z.MODEL, "messages": messages, "max_tokens": 1, "temperature": 0.0,
            "chat_template": z.FEWSHOT_TEMPLATE, "add_generation_prompt": False,
            "continue_final_message": True, "prompt_logprobs": 0}
    import urllib.request
    req = urllib.request.Request(z.URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
    out = json.load(urllib.request.urlopen(req, timeout=60))
    lps = [next(iter(d.values()))["logprob"] for d in out["prompt_logprobs"] if d]
    # The candidate is the tail of the prompt; count its tokens from the end.
    n = len(tok(PREFIX + candidate)) - len(tok(PREFIX))
    return sum(lps[-n:])


_cache = {}


def tok(text):
    if text not in _cache:
        import urllib.request
        body = {"model": z.MODEL, "prompt": text, "add_special_tokens": False}
        req = urllib.request.Request(z.URL.replace("/v1/chat/completions", "/tokenize"),
                                     json.dumps(body).encode(), {"Content-Type": "application/json"})
        _cache[text] = json.load(urllib.request.urlopen(req, timeout=30))["tokens"]
    return _cache[text]


def main():
    for label, context in [("no context", ""), ("code context", z.CONTEXT)]:
        top1, times = 0, []
        for i, cands in CANDIDATES.items():
            path = os.path.join(SCRATCH, f"case-{i:02d}.wav")
            t = time.time()
            scores = [score(path, c, context) for c in cands]
            times.append(time.time() - t)
            best = max(range(len(cands)), key=scores.__getitem__)
            top1 += best == 0
            mark = "ok " if best == 0 else "BAD"
            print(f"{label:12} {mark} {cands[best]!r:42} " +
                  " ".join(f"{s:7.1f}" for s in scores))
        ms = sorted(times)[len(times) // 2] * 1000
        print(f"{label}: top-1 {top1}/{len(CANDIDATES)}, median {ms:.0f} ms for "
              f"{sum(map(len, CANDIDATES.values())) / len(CANDIDATES):.1f} sequential candidates\n")


if __name__ == "__main__":
    main()
