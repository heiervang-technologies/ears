"""A translator backend that knows the answers: each utterance in the task
scripts maps to its step's ideal effect (@do). Same JSON-lines protocol as
`dictate.py --serve`:

    in:  {"id", "lang", "text", "scope", "mode"}          one per line
    out: {"id", "keys": "..."} or {"id", "lines": [{"code", "shift"}]}

Run through the bench, it shows the ceiling a perfect translator reaches
through this plugin (what is left is plugin or ears-command behaviour), and
with audio, what a perfect translator loses to the ASR. A natural phrase
that stands for several strict steps answers with their keys joined.

    python3 bench/run.py --translator 'python3 bench/backends/oracle.py' --label oracle
"""
import json
import os
import re
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from run import load_tasks  # noqa: E402


def key(text):
    return " ".join(re.sub(r"[^a-z0-9 ]", " ", text.lower()).split())


answers = {}
# The bench names the task being played (cursor targets differ per task).
for task in load_tasks([os.environ["EARS_BENCH_TASK"]] if os.environ.get("EARS_BENCH_TASK") else None):
    group = []
    for step in task["steps"]:
        do = step["do"]
        answers.setdefault(key(step["strict"]), do)
        if step["natural"] == "-":
            group.append(do)
            continue
        if step["natural"]:
            joined = group + [do]
            if all("keys" in d for d in joined):
                do = {"keys": "".join(d["keys"] for d in joined)}
            answers.setdefault(key(step["natural"]), do)
        group = []

for request in sys.stdin:
    r = json.loads(request)
    do = answers.get(key(r["text"]))
    reply = {"id": r["id"]}
    if do and "keys" in do:
        reply["keys"] = do["keys"]
    elif do and r.get("mode") not in ("normal", "visual"):
        reply["lines"] = do["lines"]
    elif r.get("mode") not in ("normal", "visual"):
        reply["lines"] = []
    else:
        reply["keys"] = None
    print(json.dumps(reply), flush=True)
