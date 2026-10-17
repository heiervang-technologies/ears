"""End-to-end dictation benchmark: scripted utterances through the real
Neovim plugin (ears_code.lua) in headless Neovim, scored against expected
checkpoints and a target file.

    python3 bench/run.py                        # text utterances only
    python3 bench/run.py --audio                # plus TTS -> live ASR, 2 voices
    python3 bench/run.py --translator 'python3 my_backend.py' --label lora

Inputs:   text (the script's words, isolates the translator) and, with
          --audio, each utterance synthesised by Kokoro in each voice, then
          transcribed by Qwen3-ASR with the buffer's names as context.
Variants: strict (phrasing per the README grammar) and natural (how people
          say it); see tasks/*/script.txt.
Runs:     chain (no help: the final file is the end-to-end result) and
          resync (the buffer is reset to each full checkpoint, so every
          segment is scored on its own; per-utterance numbers come from it).
"""
import argparse
import ast
import hashlib
import io
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import tokenize
import tomllib
import urllib.request

BENCH = os.path.dirname(os.path.abspath(__file__))
PLUGIN = os.path.join(BENCH, "..", "ears_code.lua")
KC3 = "/run/media/me/storage"
KC3_UUID = "0c2db6de-b82a-4277-aa2c-0b2782373ba5"
CACHE = os.path.join(KC3, "builds/ears/code-dictation-bench")
TTS_URL = "http://localhost:30383/v1/audio/speech"
# Kokoro is a shared service (it OOMed once under a burst from this bench),
# so one Kokoro voice, throttled, and one local espeak-ng voice.
VOICES = ["af_heart", "espeak:en-us+m3"]
TTS_PAUSE_S = 1.0
READONLY = False
ASR_CACHE = None  # --asr-cache: read cached clips, write nothing on KC3
CURSOR = "▮"
# How the TTS should say what is written: it reads "def" as letters.
SAY = {"def": "deff", "del": "dell", "argparse": "arg parse", "ArgumentParser": "argument parser",
       "dataclasses": "data classes", "dataclass": "data class", "len": "len"}


# --- tasks -------------------------------------------------------------------

def parse_script(path):
    """script.txt -> (start, steps). A step: {"strict", "natural", "checks"}.
    Checks: ("line", text) ("row", n) ("cursor", (r, c)) ("mode", m)
    ("lines", n) ("buffer", (lines, row, col))."""
    start, steps, block = None, [], None
    for raw in open(path).read().split("\n"):
        if block is not None:
            if raw == "===":
                lines, row, col = [], None, None
                for k, line in enumerate(block):
                    if CURSOR in line:
                        row, col = k + 1, line.index(CURSOR)
                        line = line.replace(CURSOR, "")
                    lines.append(line)
                steps[-1]["checks"].append(("buffer", (lines, row, col)))
                block = None
            else:
                block.append(raw)
            continue
        if raw.startswith("#") or not raw.strip():
            continue
        if raw == "===":
            block = []
        elif raw.startswith("@start "):
            pos, mode = raw.split()[1:]
            r, c = pos.split(":")
            start = {"row": int(r), "col": int(c), "mode": mode}
        elif raw.startswith("> "):
            steps.append({"strict": raw[2:].strip(), "natural": None, "checks": []})
        elif raw.startswith("~ "):
            steps[-1]["natural"] = raw[2:].strip()
        elif raw.startswith(("@row ", "@lines ")):
            kind, n = raw[1:].split()
            steps[-1]["checks"].append((kind, int(n)))
        elif raw == "@line" or raw.startswith("@line "):
            steps[-1]["checks"].append(("line", raw[6:]))
        elif raw.startswith("@cursor "):
            r, c = raw.split()[1].split(":")
            steps[-1]["checks"].append(("cursor", (int(r), int(c))))
        elif raw.startswith("@do keys "):
            steps[-1]["do"] = {"keys": raw[len("@do keys "):]}
        elif raw.startswith("@do line"):
            shift, _, code = raw[len("@do line "):].partition(" ")
            do = steps[-1].setdefault("do", {"lines": []})
            do["lines"].append({"shift": int(shift), "code": code})
        elif raw.startswith("@mode "):
            steps[-1]["checks"].append(("mode", raw.split()[1]))
        else:
            raise SystemExit(f"{path}: cannot parse {raw!r}")
    return start, steps


def load_tasks(names=None):
    tasks = []
    for name in sorted(os.listdir(os.path.join(BENCH, "tasks"))):
        if names and not any(n in name for n in names):
            continue
        d = os.path.join(BENCH, "tasks", name)
        start, steps = parse_script(os.path.join(d, "script.txt"))
        tasks.append({"name": name, "dir": d, "start": start, "steps": steps,
                      "start_lines": open(os.path.join(d, "start.py")).read().rstrip("\n").split("\n"),
                      "target": open(os.path.join(d, "target.py")).read().rstrip("\n").split("\n")})
    return tasks


def variant_steps(task, variant, golden=None):
    """The steps said in a variant. With the golden run's states: `before`,
    the state to start each step from, and `want`, the state it should end in."""
    out, prev = [], None
    for index, step in enumerate(task["steps"]):
        text = step["natural"] if variant == "natural" and step["natural"] else step["strict"]
        if text == "-":
            continue
        before = golden[prev] if golden and prev is not None else None
        out.append({"index": index, "text": text, "checks": step["checks"], "do": step.get("do"),
                    "before": before, "want": golden[index] if golden else None,
                    "mode_before": before["mode"] if before else task["start"]["mode"]})
        prev = index
    return out


def golden_run(task, tmp):
    """Play every step's ideal effect (@do) to get the expected state after
    each step, and check the script's own checks against it."""
    steps = variant_steps(task, "strict")
    missing = [s["index"] + 1 for s in steps if not s["do"]]
    if missing:
        raise SystemExit(f"{task['name']}: steps without @do: {missing}")
    snaps = run_job(task, steps, False, None, tmp, golden=True)
    problems = [f"{task['name']}#{s['index'] + 1}: golden state fails its {k} check"
                for s, snap in zip(steps, snaps) for k, v in s["checks"] if not check(k, v, snap)]
    final = final_scores(snaps[-1]["lines"], task["target"])
    if not final["lenient"]:  # blank lines may keep the plugin's indentation
        problems.append(f"{task['name']}: golden final file differs from target.py")
    return {s["index"]: snap for s, snap in zip(steps, snaps)}, problems


# --- audio ---------------------------------------------------------------------

def kc3_ready(readonly=False):
    uuid = subprocess.run(["findmnt", "--mountpoint", KC3, "-n", "-o", "UUID"],
                          capture_output=True, text=True).stdout.strip()
    if uuid != KC3_UUID:
        raise SystemExit("KC3 is not mounted at " + KC3 + "; not writing the audio cache")
    if readonly:
        return
    free = shutil.disk_usage(KC3).free / 1e9
    if free < 30:
        raise SystemExit(f"KC3 has {free:.0f} GB free (< 30 GB); stopping")
    os.makedirs(os.path.join(CACHE, "audio"), exist_ok=True)


def spoken_form(text):
    return " ".join(SAY.get(w, w) for w in text.split())


_restarts = []


def kokoro_guard():
    """Stop if the Kokoro pod restarted since the run began."""
    out = subprocess.run(["kubectl", "-n", "ai", "get", "pods", "-o",
                          "jsonpath={range .items[*]}{.metadata.name} {.status.containerStatuses[0].restartCount}{'\\n'}{end}"],
                         capture_output=True, text=True).stdout
    count = sum(int(l.split()[1]) for l in out.splitlines() if l.startswith("kokoro") and len(l.split()) == 2)
    if not out:
        return
    _restarts.append(count)
    if count != _restarts[0]:
        raise SystemExit("Kokoro restarted during this run; stopping TTS requests")


def synthesize(text, voice):
    """Cached 16 kHz mono WAV of `text` in `voice`."""
    key = hashlib.sha1(f"kokoro|{voice}|{spoken_form(text)}".encode()).hexdigest()[:16]
    path = os.path.join(CACHE, "audio", voice, key + ".wav")
    if os.path.exists(path):
        return path
    if READONLY:
        raise SystemExit(f"--asr-cache: no cached clip for {voice}: {text!r}")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if voice.startswith("espeak:"):
        subprocess.run(["espeak-ng", "-v", voice.split(":", 1)[1], "-s", "150", "-w", path + ".raw.wav",
                        spoken_form(text)], check=True)
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", path + ".raw.wav", "-ar", "16000", "-ac", "1",
                        path + ".part.wav"], check=True)
        os.remove(path + ".raw.wav")
        os.replace(path + ".part.wav", path)
        return path
    kokoro_guard()
    time.sleep(TTS_PAUSE_S)
    body = {"model": "kokoro", "input": spoken_form(text), "voice": voice, "response_format": "wav"}
    req = urllib.request.Request(TTS_URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
    raw = urllib.request.urlopen(req, timeout=120).read()
    with tempfile.NamedTemporaryFile(suffix=".wav", dir=os.path.dirname(path)) as tmp:
        tmp.write(raw)
        tmp.flush()
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", tmp.name, "-ar", "16000", "-ac", "1",
                        path + ".part.wav"], check=True)
    os.replace(path + ".part.wav", path)
    return path


# --- running -------------------------------------------------------------------

def ears_commands():
    """The [commands] section of the user's ears config, with ears' defaults."""
    conf = {"enabled": False, "enter": ["enter"], "new_line": ["new line"], "undo": ["scratch that"],
            "back": ["back"], "backtrack": ["backtrack", "back track"], "literal": ["literal"], "keys": {}}
    path = os.path.expanduser("~/.config/ears/config.toml")
    if os.path.exists(path):
        with open(path, "rb") as f:
            conf.update(tomllib.load(f).get("commands", {}))
    return conf


def run_job(task, steps, resync, translator, tmp, timeout_ms=8000, golden=False):
    out = os.path.join(tmp, "result.json")
    if os.path.exists(out):
        os.remove(out)
    job = {"plugin": os.path.abspath(PLUGIN), "hear": os.path.join(BENCH, "hear.py"),
           "buf_name": os.path.join(tmp, "work", task["name"] + ".py"), "filetype": "python",
           "start_lines": task["start_lines"], "start": task["start"], "steps": steps, "resync": resync,
           "commands": ears_commands(), "timeout_ms": timeout_ms, "out": out, "golden": golden}
    path = os.path.join(tmp, "job.json")
    json.dump(job, open(path, "w"))
    env = {**os.environ, "EARS_CODE_HEADLESS": "1", "EARS_BENCH_JOB": path, "EARS_BENCH_CACHE": ASR_CACHE or CACHE,
           "EARS_BENCH_TASK": task["name"], "XDG_RUNTIME_DIR": tmp}  # nothing here can reach the live ears sockets
    for var in ("NVIM", "NVIM_LISTEN_ADDRESS"):  # never the user's Neovim sockets
        env.pop(var, None)
    if translator:
        env["EARS_CODE_TRANSLATOR"] = translator
    cmd = ["nvim", "--headless", "-n", "-u", "NONE", "-i", "NONE",
           "-c", "luafile " + os.path.join(BENCH, "driver.lua")]
    budget = 30 + len(steps) * (timeout_ms / 1000 + 15)
    proc = subprocess.run(cmd, env=env, cwd=tmp, stdin=subprocess.DEVNULL, capture_output=True,
                          text=True, timeout=budget)  # on timeout run() kills nvim by PID
    if not os.path.exists(out):
        raise RuntimeError(f"{task['name']}: no result; stderr: {proc.stderr[-2000:]}")
    result = json.load(open(out))
    if "fatal" in result:
        raise RuntimeError(f"{task['name']}: {result['fatal']}")
    return result["steps"]


# --- scoring -------------------------------------------------------------------

def lenient(line):
    """Indentation, then the line's Python tokens: spacing between tokens
    does not count (`f(a = 1)` == `f(a=1)`), the tokens do (`* *` != `**`).
    Lines that do not tokenize on their own fall back to collapsed spaces."""
    indent = re.match(r"\s*", line).group()
    rest = line[len(indent):].rstrip()
    if not rest:
        return ""
    try:
        toks = [t.string for t in tokenize.generate_tokens(io.StringIO(rest + "\n").readline)
                if t.type not in (tokenize.NEWLINE, tokenize.NL, tokenize.ENDMARKER)]
        return indent + " ".join(toks)
    except (tokenize.TokenError, SyntaxError, IndentationError):
        return indent + re.sub(r"\s+", " ", rest)


def check(kind, val, snap):
    lines, row = snap["lines"], snap["row"]
    current = lines[row - 1] if 0 < row <= len(lines) else ""
    if kind == "line":
        return lenient(current) == lenient(val)
    if kind == "row":
        return row == val
    if kind == "cursor":
        return (row, snap["col"]) == val
    if kind == "mode":
        return snap["mode"] == val
    if kind == "lines":
        return len(lines) == val
    if kind == "buffer":
        want, wrow, wcol = val
        same = [lenient(a) for a in lines] == [lenient(b) for b in want]
        if wrow is None:
            return same
        return same and row == wrow  # the column only places the resync
    raise ValueError(kind)


def exact(kind, val, snap):
    lines, row = snap["lines"], snap["row"]
    if kind == "line":
        return 0 < row <= len(lines) and lines[row - 1] == val
    if kind == "buffer":
        return lines == val[0]
    return check(kind, val, snap)


def ruff_format(lines):
    if not shutil.which("ruff"):
        return None
    p = subprocess.run(["ruff", "format", "--stdin-filename", "x.py", "-"], input="\n".join(lines) + "\n",
                       capture_output=True, text=True)
    return p.stdout if p.returncode == 0 else None


def final_scores(lines, target):
    def strip(ls):
        ls = list(ls)
        while ls and not ls[-1].strip():
            ls.pop()
        return ls
    got, want = strip(lines), strip(target)
    try:
        same_ast = ast.dump(ast.parse("\n".join(got))) == ast.dump(ast.parse("\n".join(want)))
    except SyntaxError:
        same_ast = False
    formatted = ruff_format(got)
    return {"exact": got == want, "lenient": [lenient(a) for a in got] == [lenient(b) for b in want],
            "ruff": formatted is not None and formatted == ruff_format(want), "ast": same_ast,
            "parses": _parses(got)}


def _parses(lines):
    try:
        ast.parse("\n".join(lines))
        return True
    except SyntaxError:
        return False


NUMBER_WORDS = {w: str(i) for i, w in enumerate(
    "zero one two three four five six seven eight nine ten eleven twelve".split())}


def norm_words(text):
    """Words for comparing what was said with what was heard: lowercase, no
    punctuation, numbers as digits."""
    words = re.sub(r"[^a-z0-9_ ]", " ", text.lower().replace("-", " ")).split()
    return [NUMBER_WORDS.get(w, w) for w in words]


def word_errors(said, heard):
    """Word-level edit distance."""
    a, b = norm_words(said), norm_words(heard)
    d = list(range(len(b) + 1))
    for i in range(1, len(a) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(b) + 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (a[i - 1] != b[j - 1]))
    return d[len(b)], len(a)


def category(step, snap, text_passed):
    """A first guess at why a step failed; RESULTS.md has the triage."""
    if snap.get("asr_ms") is not None and (text_passed or snap["heard_errors"]):
        return "asr" if text_passed else "asr+text-fails-too"
    if snap.get("error"):
        return "plugin-error"
    msgs = " ".join(snap["messages"])
    if "not a command" in msgs:
        return "command-gap"
    if "not found" in msgs:
        return "target-not-found"
    if snap["event"].startswith("VoiceCommand"):
        return "ears-command"
    return "translator" if step["mode_before"] == "i" else "command"


def score_run(task, steps, snaps, resync=False):
    """Per-step results. A failure after an earlier failure in the same
    segment (since the last resync) is marked a cascade: its own utterance
    may be fine, it started from a wrong buffer."""
    rows, broken = [], False
    for step, snap in zip(steps, snaps):
        want = step["want"]
        diff = [k for k, ok in (("text", [lenient(a) for a in snap["lines"]] == [lenient(b) for b in want["lines"]]),
                                ("row", snap["row"] == want["row"]), ("mode", snap["mode"] == want["mode"]),
                                ("col", snap["lines"] != want["lines"] or snap["row"] != want["row"]
                                 or snap["col"] == want["col"] or not want["lines"][want["row"] - 1].strip()))
                if not ok]
        rows.append({"index": step["index"], "said": step["text"], "heard": snap["heard"],
                     "event": snap["event"], "pass": not diff, "exact": not diff and snap["lines"] == want["lines"],
                     "failed_checks": diff,
                     "line": (snap["lines"][snap["row"] - 1] if 0 < snap["row"] <= len(snap["lines"]) else None),
                     "mode": snap["mode"], "row": snap["row"], "col": snap["col"],
                     "asr_ms": snap.get("asr_ms"), "asr_cached": snap.get("asr_cached"),
                     "translate_ms": snap.get("translate_ms"), "messages": snap["messages"],
                     "error": snap.get("error"), "mode_before": step["mode_before"],
                     "heard_errors": word_errors(step["text"], snap["heard"])[0] if snap.get("asr_ms") is not None else None,
                     "said_words": len(norm_words(step["text"]))})
        rows[-1]["cascade"] = broken and not rows[-1]["pass"]
        broken = broken or not rows[-1]["pass"]
        if resync:
            broken = False  # every step starts from the ideal state
    return rows


# --- main ------------------------------------------------------------------------

def pct(xs):
    return f"{100 * sum(xs) / len(xs):.0f}%" if xs else "-"


def ms_stats(xs):
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return "-"
    p90 = xs[min(len(xs) - 1, int(0.9 * len(xs)))]
    return f"{statistics.median(xs):.0f} / {p90:.0f} ms (n={len(xs)})"


def report(results, label):
    out = [f"# Benchmark: {label}", ""]
    out.append("| input | variant | steps passed (resync) | exact | failed | final exact | final ruff | final AST | parses |")
    out.append("|---|---|---|---|---|---|---|---|---|")
    groups = {}
    for r in results:
        groups.setdefault((r["input"], r["variant"]), []).append(r)
    for (inp, var), rs in groups.items():
        steps = [s for r in rs for s in r["resync"]]
        finals = [r["final"] for r in rs]
        out.append(f"| {inp} | {var} | {sum(s['pass'] for s in steps)}/{len(steps)} ({pct([s['pass'] for s in steps])}) "
                   f"| {pct([s['exact'] for s in steps])} | {sum(not s['pass'] for s in steps)} "
                   f"| {sum(f['exact'] for f in finals)}/{len(finals)} | {sum(f['ruff'] for f in finals)}/{len(finals)} "
                   f"| {sum(f['ast'] for f in finals)}/{len(finals)} | {sum(f['parses'] for f in finals)}/{len(finals)} |")
    out += ["", "## Per task (resync steps passed; final AST-equal in the chain run)", ""]
    inputs = sorted({k[0] for k in groups}, key=lambda i: (i != "text", i))
    head = "| task | " + " | ".join(f"{i} {v}" for i in inputs for v in ("strict", "natural")) + " |"
    out += [head, "|---|" + "---|" * (2 * len(inputs))]
    for name in sorted({r["task"] for r in results}):
        cells = []
        for i in inputs:
            for v in ("strict", "natural"):
                r = next((r for r in results if r["task"] == name and r["input"] == i and r["variant"] == v), None)
                cells.append("-" if not r else f"{sum(s['pass'] for s in r['resync'])}/{len(r['resync'])}"
                             f"{' ✓' if r['final']['ast'] else ''}")
        out.append(f"| {name} | " + " | ".join(cells) + " |")
    out += ["", "## Latency (median / p90)", ""]
    asr = [s["asr_ms"] for r in results for s in r["resync"] + r["chain"] if s["asr_ms"] is not None and not s["asr_cached"]]
    trans = [s["translate_ms"] for r in results for s in (r["resync"] + r["chain"])[1:] if s["translate_ms"] is not None]
    first = [r["chain"][0]["translate_ms"] for r in results if r["chain"] and r["chain"][0]["translate_ms"]]
    out.append(f"- ASR (uncached requests, with context): {ms_stats(asr)}")
    for inp in inputs:
        if inp == "text":
            continue
        ss = [s for r in results if r["input"] == inp for s in r["resync"]]
        wer = sum(s["heard_errors"] for s in ss) / max(1, sum(s["said_words"] for s in ss))
        ok = [s["pass"] for s in ss if s["heard_errors"] == 0]
        out.append(f"- {inp}: word error rate {100 * wer:.0f}%, utterances heard word-perfect "
                   f"{sum(s['heard_errors'] == 0 for s in ss)}/{len(ss)}; of those, passed {sum(ok)}/{len(ok)}")
    out.append(f"- translate + apply (event to translator reply, excluding each run's first): {ms_stats(trans)}")
    out.append(f"- first utterance of a run (translator start-up): {ms_stats(first)}")
    out += ["", "## Failed steps (resync run; cascades counted, not listed)", ""]
    cats = {}
    for r in results:
        for s in r["resync"]:
            if not s["pass"]:
                cats[s["category"]] = cats.get(s["category"], 0) + 1
    out.append(", ".join(f"{k}: {v}" for k, v in sorted(cats.items(), key=lambda kv: -kv[1])) or "none")
    out.append("")
    for r in results:
        for s in r["resync"]:
            if not s["pass"] and not s["cascade"]:
                heard = "" if r["input"] == "text" else f" heard {s['heard']!r}"
                msg = f" [{'; '.join(s['messages'])[:120]}]" if s["messages"] else ""
                out.append(f"- `{r['task']}#{s['index'] + 1}` {r['input']}/{r['variant']} ({s['category']}): "
                           f"said {s['said']!r}{heard} → line {s['line']!r}, mode {s['mode']}, "
                           f"failed {','.join(s['failed_checks'])}{msg}")
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--audio", action="store_true", help="also run TTS -> ASR for each voice")
    ap.add_argument("--audio-only", action="store_true", help="skip the text input")
    ap.add_argument("--voices", default=",".join(VOICES))
    ap.add_argument("--variants", default="strict,natural")
    ap.add_argument("--tasks", default="", help="comma-separated substrings of task names")
    ap.add_argument("--translator", default=os.environ.get("EARS_CODE_TRANSLATOR"),
                    help="shell command serving dictate.py's --serve JSON lines (default: dictate.py)")
    ap.add_argument("--asr-cache", help="keep transcripts here and only read cached audio from KC3")
    ap.add_argument("--label", default="baseline")
    ap.add_argument("--out", default=os.path.join(BENCH, "results"))
    args = ap.parse_args()

    tasks = load_tasks([t for t in args.tasks.split(",") if t])
    inputs = [] if args.audio_only else ["text"]
    global READONLY, ASR_CACHE
    if args.asr_cache:
        READONLY, ASR_CACHE = True, args.asr_cache
        os.makedirs(ASR_CACHE, exist_ok=True)
    if args.audio or args.audio_only:
        kc3_ready(READONLY)
        inputs += args.voices.split(",")
    results = []
    with tempfile.TemporaryDirectory(prefix="ears-bench-") as tmp:
        os.makedirs(os.path.join(tmp, "work"))
        for inp in inputs:
            for task in tasks:
                if "golden" not in task:
                    task["golden"], problems = golden_run(task, tmp)
                    for p in problems:
                        print("HARNESS: " + p, file=sys.stderr)
                for variant in args.variants.split(","):
                    steps = variant_steps(task, variant, task["golden"])
                    if inp != "text":
                        for s in steps:
                            s["wav"] = synthesize(s["text"], inp)
                    t = time.time()
                    runs = {}
                    for mode in ("chain", "resync"):
                        snaps = run_job(task, steps, mode == "resync", args.translator, tmp)
                        runs[mode] = (score_run(task, steps, snaps, mode == "resync"), snaps[-1]["lines"])
                    results.append({"task": task["name"], "input": inp, "variant": variant,
                                    "chain": runs["chain"][0], "resync": runs["resync"][0],
                                    "final_lines": runs["chain"][1],
                                    "final": final_scores(runs["chain"][1], task["target"])})
                    r = results[-1]
                    print(f"{inp:10} {variant:8} {task['name']:18} resync {sum(s['pass'] for s in r['resync'])}"
                          f"/{len(r['resync'])}  final {r['final']}  {time.time() - t:.1f}s", file=sys.stderr)
    text_pass = {(r["task"], r["variant"], s["index"]): s["pass"]
                 for r in results if r["input"] == "text" for s in r["resync"]}
    for r in results:
        task = next(t for t in tasks if t["name"] == r["task"])
        steps = {s["index"]: s for s in variant_steps(task, r["variant"], task["golden"])}
        for s in r["resync"] + r["chain"]:
            s["category"] = None if s["pass"] else "cascade" if s["cascade"] else category(
                steps[s["index"]], {**s, "event": s["event"]},
                r["input"] != "text" and text_pass.get((r["task"], r["variant"], s["index"]), False))
    os.makedirs(args.out, exist_ok=True)
    meta = {"label": args.label, "translator": args.translator or "dictate.py --serve",
            "inputs": inputs, "date": time.strftime("%Y-%m-%d %H:%M")}
    json.dump({"meta": meta, "results": results}, open(os.path.join(args.out, args.label + ".json"), "w"), indent=1)
    md = report(results, args.label)
    open(os.path.join(args.out, args.label + ".md"), "w").write(md)
    print(md)


if __name__ == "__main__":
    main()
