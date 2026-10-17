"""Can prompting alone make Qwen3-ASR write code from spoken code?

    python zero_shot_probe.py [scratch_dir]

espeak-ng speaks each case; each clip is decoded sequentially under five
conditions:

  plain     no context (stock template)
  ctx       the preceding code as the system context (stock template)
  fewshot   three earlier (audio, code) turns as a real multi-turn prompt
            (per-request chat template, needs --trust-request-chat-template)
  fewshot+  fewshot plus the preceding code as context
  prefill   ctx, with the assistant turn forced to start a code line
            ("language English<asr_text>" + the case's first written token)

Scoring is exact match with the expected line, after collapsing whitespace.
"""

import base64, json, os, re, subprocess, sys, time, urllib.request

URL = "http://localhost:30189/v1/chat/completions"
MODEL = "Qwen/Qwen3-ASR-1.7B"
SCRATCH = sys.argv[1] if len(sys.argv) > 1 else "/tmp/zsp"

# (spoken, expected code)
CASES = [
    ("for i in range ten", "for i in range(10):"),
    ("print of string hello comma x", 'print("hello", x)'),
    ("def parse args of argv", "def parse_args(argv):"),
    ("if x is none", "if x is None:"),
    ("return self dot items index key", "return self.items[key]"),
    ("import numpy as np", "import numpy as np"),
    ("total equals sum of sizes", "total = sum(sizes)"),
    ("while not done", "while not done:"),
    ("x equals len of names", "x = len(names)"),
    ("class request handler", "class RequestHandler:"),
    ("args equals parse args of argv", "args = parse_args(argv)"),
    ("result dot append of item", "result.append(item)"),
    ("with open of path as f", "with open(path) as f:"),
    ("return none", "return None"),
    ("for key comma value in data dot items", "for key, value in data.items():"),
    ("if name equals equals string main", 'if __name__ == "__main__":'),
    ("raise value error of string bad input", 'raise ValueError("bad input")'),
    ("count plus equals one", "count += 1"),
    ("lines equals f dot read of close dot split lines", "lines = f.read().splitlines()"),
    ("print of f string total colon brace total", 'print(f"total: {total}")'),
]

# Preceding code, as an editor would send it (names in scope).
CONTEXT = '''import sys
import numpy as np

class RequestHandler:
    def __init__(self, items):
        self.items = items

def parse_args(argv):
    names = [a for a in argv if not a.startswith("-")]
    sizes = [len(n) for n in names]
    data = {"names": names}
    path = argv[0]
    result = []
    done = False
    count = 0
    total = 0
'''

# Few-shot turns: spoken -> code. None of these are in CASES.
SHOTS = [
    ("for name in names", "for name in names:"),
    ("y equals max of a comma b", "y = max(a, b)"),
    ("def main of", "def main():"),
]

# Multi-turn template: system, then each user audio followed by its assistant
# reply; the last assistant message (if any) is left open as a prefix.
FEWSHOT_TEMPLATE = "".join([
    "{{- '<|im_start|>system\\n' + (messages[0].content if messages[0].role == 'system' else '') + '<|im_end|>\\n' -}}",
    "{%- for m in messages -%}",
    "{%- if m.role == 'user' -%}",
    "{{- '<|im_start|>user\\n<|audio_start|><|audio_pad|><|audio_end|><|im_end|>\\n' -}}",
    "{%- elif m.role == 'assistant' -%}",
    "{{- '<|im_start|>assistant\\n' + m.content -}}",
    "{%- if not loop.last -%}{{- '<|im_end|>\\n' -}}{%- endif -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
])


def tts(text, path):
    if os.path.exists(path):
        return
    subprocess.run(["espeak-ng", "-s", "150", "-w", path + ".raw.wav", text], check=True)
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", path + ".raw.wav",
                    "-ar", "16000", "-ac", "1", path], check=True)
    os.remove(path + ".raw.wav")


def audio_msg(path):
    data = base64.b64encode(open(path, "rb").read()).decode()
    return {"role": "user", "content": [
        {"type": "input_audio", "input_audio": {"data": data, "format": "wav"}}]}


def post(body):
    req = urllib.request.Request(URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
    t = time.time()
    out = json.load(urllib.request.urlopen(req, timeout=60))
    return out["choices"][0]["message"]["content"], time.time() - t


def decode(path, system=None, shots=None, prefix=None):
    messages = [{"role": "system", "content": system or ""}]
    body = {"model": MODEL, "max_tokens": 64, "temperature": 0.0}
    if shots is None and prefix is None:
        messages.append(audio_msg(path))
        if not system:
            messages = messages[1:]
        text, dt = post({**body, "messages": messages})
        return text.split("<asr_text>", 1)[-1].strip(), dt
    for spoken, code in shots or []:
        messages.append(audio_msg(shot_path(spoken)))
        messages.append({"role": "assistant", "content": "language English<asr_text>" + code})
    messages.append(audio_msg(path))
    start = "language English<asr_text>" + (prefix or "")
    messages.append({"role": "assistant", "content": start})
    text, dt = post({**body, "messages": messages, "chat_template": FEWSHOT_TEMPLATE,
                     "add_generation_prompt": False, "continue_final_message": True})
    return ((prefix or "") + text).strip(), dt


def shot_path(spoken):
    return os.path.join(SCRATCH, "shot-" + re.sub(r"\W+", "_", spoken) + ".wav")


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


def main():
    os.makedirs(SCRATCH, exist_ok=True)
    for spoken, _ in SHOTS:
        tts(spoken, shot_path(spoken))
    conds = {
        "plain": lambda p, code: decode(p),
        "ctx": lambda p, code: decode(p, CONTEXT),
        "fewshot": lambda p, code: decode(p, None, SHOTS),
        "fewshot+": lambda p, code: decode(p, CONTEXT, SHOTS),
        "prefill": lambda p, code: decode(p, CONTEXT, [], re.match(r"\w+|\W", code).group(0)),
    }
    hits = {c: 0 for c in conds}
    lat = {c: [] for c in conds}
    rows = []
    for i, (spoken, code) in enumerate(CASES):
        p = os.path.join(SCRATCH, f"case-{i:02d}.wav")
        tts(spoken, p)
        row = {"spoken": spoken, "expected": code}
        for name, fn in conds.items():
            out, dt = fn(p, code)
            row[name] = out
            lat[name].append(dt)
            hits[name] += norm(out) == norm(code)
        rows.append(row)
        print(json.dumps(row), flush=True)
    print()
    for name in conds:
        ms = sorted(lat[name])[len(lat[name]) // 2] * 1000
        print(f"{name:9} exact {hits[name]:2}/{len(CASES)}  median {ms:4.0f} ms")
    json.dump(rows, open(os.path.join(SCRATCH, "results.json"), "w"), indent=1)


if __name__ == "__main__":
    main()
