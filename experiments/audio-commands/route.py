"""Can EmbeddingGemma 2 route short spoken ears commands straight from audio?

Phases (each cached under WORK so the analysis can be re-run cheaply):
  clips   espeak-ng in three voices + a few Kokoro clips, all 16 kHz mono wav
  embed   audio and text embeddings from the embeddinggemma2 llama.cpp server
  asr     Qwen3-ASR transcripts of the same clips (the transcript route)
  report  accuracy / threshold / latency tables (markdown on stdout)

Usage: python3 route.py [clips|embed|asr|report|all]
"""
import base64, json, os, re, subprocess, sys, time, urllib.request

import numpy as np

WORK = "/tmp/acmd"
EMB_URL = "http://localhost:30201/v1/embeddings"
ASR_URL = "http://localhost:30189/v1/chat/completions"
ASR_MODEL = "Qwen/Qwen3-ASR-1.7B"
KOKORO_URL = "http://localhost:30383/v1/audio/speech"
# llama.cpp mtmd placeholder, from GET /props "media_marker"
MEDIA = "<__media_JWjkcQPZggi6fKEcWUQ9MaL6JdeGXdVg__>"

# Text exemplars per class (2-4 phrasings each). The first phrasings follow
# ~/.config/ears/config.toml [commands]; the rest are plausible variants.
COMMANDS = {
    "enter": ["enter", "over", "press enter", "submit"],
    "new_line": ["new line", "next line", "line break"],
    "undo": ["scratch that", "undo that", "delete that"],
    "back_three": ["back three", "back three words", "delete three words"],
    "sleep": ["ears off", "fuck off clanker", "stop listening"],
    "wake": ["hey clanker", "ears on", "listen clanker"],
    "clear": ["clear", "erase", "clear the line"],
    "go_back": ["go back", "navigate back", "previous page"],
    "scroll_down": ["scroll down", "page down", "scroll lower"],
    "menu": ["menu", "open the menu", "show menu"],
    "search_bar": ["search bar", "focus the search bar", "go to search"],
    "workspace_three": ["workspace three", "go to workspace three", "switch to workspace three"],
}

# What gets spoken. Commands: (class, utterance). Utterances are mostly the
# exemplar phrasings, plus a few unseen variants marked by not being exemplars.
SPOKEN_COMMANDS = [
    ("enter", "enter"), ("enter", "over"),
    ("new_line", "new line"),
    ("undo", "scratch that"),
    ("back_three", "back three"),
    ("sleep", "ears off"), ("sleep", "fuck off clanker"),
    ("wake", "hey clanker"), ("wake", "ears on"),
    ("clear", "clear"), ("clear", "erase"),
    ("go_back", "go back"),
    ("scroll_down", "scroll down"),
    ("menu", "menu"),
    ("search_bar", "search bar"),
    ("workspace_three", "workspace three"),
    # unseen variants
    ("scroll_down", "scroll down a bit"),
    ("workspace_three", "switch to workspace 3"),
]
NON_COMMANDS = [
    "okay", "yeah", "over there", "enter the room", "hello world", "I think so",
    "go ahead", "clear skies today", "back then", "the menu was great",
    "three more", "thanks a lot", "let me check", "no worries",
]

# espeak voices: A is only used as the few-shot audio exemplar bank,
# B and C (and Kokoro) are the queries.
ESPEAK = {"A": ("en-gb-x-rp", 135), "B": ("en-us", 160), "C": ("en-us+f3", 185)}
KOKORO_VOICE = "am_adam"
KOKORO_ITEMS = [  # at most ~15 clips; the service OOMs under bursts
    ("enter", "enter"), ("enter", "over"), ("sleep", "ears off"), ("wake", "hey clanker"),
    ("undo", "scratch that"), ("clear", "erase"), ("scroll_down", "scroll down"),
    ("workspace_three", "workspace three"), ("back_three", "back three"),
    (None, "okay"), (None, "over there"), (None, "enter the room"),
    (None, "I think so"), (None, "go ahead"), (None, "yeah"),
]

TEXT_PREFIXES = {
    "none": "{}",
    "classification": "task: classification | query: {}",
    "similarity": "task: sentence similarity | query: {}",
    "document": "title: none | text: {}",
}


def slug(s):
    return re.sub(r"[^a-z0-9]+", "_", s.lower()).strip("_")


def clip_list():
    """(voice, class or None, utterance, path) for every clip."""
    out = []
    for v in ESPEAK:
        for cls, u in SPOKEN_COMMANDS:
            out.append((v, cls, u, f"{WORK}/{v}-{slug(u)}.wav"))
        for u in NON_COMMANDS:
            out.append((v, None, u, f"{WORK}/{v}-{slug(u)}.wav"))
    for cls, u in KOKORO_ITEMS:
        out.append(("K", cls, u, f"{WORK}/K-{slug(u)}.wav"))
    return out


def to16k(src, dst):
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", src,
                    "-ar", "16000", "-ac", "1", dst], check=True)
    os.remove(src)


def kokoro_restarts():
    out = subprocess.run(["kubectl", "-n", "ai", "get", "pods", "--no-headers"],
                         capture_output=True, text=True).stdout
    return [l.split()[3] for l in out.splitlines() if "kokoro" in l]


def make_clips():
    os.makedirs(WORK, exist_ok=True)
    before = kokoro_restarts()
    for v, cls, u, path in clip_list():
        if os.path.exists(path):
            continue
        if v in ESPEAK:
            voice, speed = ESPEAK[v]
            subprocess.run(["espeak-ng", "-v", voice, "-s", str(speed), "-w", path + ".raw.wav", u], check=True)
            to16k(path + ".raw.wav", path)
        else:
            if kokoro_restarts() != before:
                sys.exit("kokoro restarted; stopping")
            body = {"model": "kokoro", "input": u, "voice": KOKORO_VOICE, "response_format": "wav"}
            req = urllib.request.Request(KOKORO_URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
            open(path + ".raw.wav", "wb").write(urllib.request.urlopen(req, timeout=60).read())
            to16k(path + ".raw.wav", path)
            time.sleep(2)
    print("kokoro restarts before/after:", before, kokoro_restarts())


def embed(payload):
    req = urllib.request.Request(EMB_URL, json.dumps({"input": payload}).encode(),
                                 {"Content-Type": "application/json"})
    t = time.time()
    d = json.load(urllib.request.urlopen(req, timeout=120))
    v = np.array(d["data"][0]["embedding"], dtype=np.float32)
    return v / np.linalg.norm(v), time.time() - t


def embed_audio(path, prompt="{}"):
    b64 = base64.b64encode(open(path, "rb").read()).decode()
    return embed({"prompt_string": prompt.format(MEDIA), "multimodal_data": [b64]})


def load_cache(name):
    p = f"{WORK}/{name}.json"
    return json.load(open(p)) if os.path.exists(p) else {}


def save_cache(name, d):
    json.dump(d, open(f"{WORK}/{name}.json", "w"))


def run_embed():
    cache = load_cache("emb")
    lat = load_cache("lat")
    lat.setdefault("audio", [])
    lat.setdefault("text", [])
    embed("warm up")
    for pname, tmpl in TEXT_PREFIXES.items():
        for phrases in COMMANDS.values():
            for p in phrases:
                key = f"text|{pname}|{p}"
                if key not in cache:
                    v, dt = embed(tmpl.format(p))
                    cache[key] = v.tolist()
                    lat["text"].append(dt)
    for v_, cls, u, path in clip_list():
        for aname, tmpl in (("plain", "{}"), ("classification", "task: classification | query: {}")):
            key = f"audio|{aname}|{path}"
            if key not in cache:
                v, dt = embed_audio(path, tmpl)
                cache[key] = v.tolist()
                if aname == "plain":
                    lat["audio"].append(dt)
    save_cache("emb", cache)
    save_cache("lat", lat)


def asr(path):
    audio = base64.b64encode(open(path, "rb").read()).decode()
    body = {"model": ASR_MODEL, "max_tokens": 64, "temperature": 0.0, "messages": [
        {"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]}]}
    req = urllib.request.Request(ASR_URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
    t = time.time()
    out = json.load(urllib.request.urlopen(req, timeout=60))["choices"][0]["message"]["content"]
    return out.split("<asr_text>", 1)[-1].strip(), time.time() - t


def run_asr():
    cache = load_cache("asr")
    emb = load_cache("emb")
    lat = load_cache("lat")
    lat.setdefault("asr", [])
    for v_, cls, u, path in clip_list():
        if v_ == "A" or path in cache:
            continue
        text, dt = asr(path)
        lat["asr"].append(dt)
        cache[path] = text
        for pname, tmpl in TEXT_PREFIXES.items():
            key = f"asrtext|{pname}|{text}"
            if key not in emb:
                emb[key] = embed(tmpl.format(text))[0].tolist()
    save_cache("asr", cache)
    save_cache("emb", emb)
    save_cache("lat", lat)


def config_phrases():
    """ears' own phrase lists (exact match is what ears does today)."""
    import tomllib
    cfg = tomllib.load(open(os.path.expanduser("~/.config/ears/config.toml"), "rb"))["commands"]
    m = {norm(p): c for c, ps in COMMANDS.items() for p in ps}
    for cls, key in (("enter", "enter"), ("new_line", "new_line"), ("undo", "undo"), ("sleep", "sleep"), ("wake", "wake")):
        m.update({norm(p): cls for p in cfg.get(key, [])})
    m.update({norm(k): ("clear" if k in ("clear", "erase") else k) for k in cfg.get("keys", {})})
    return m


def norm(s):
    return re.sub(r"[^a-z0-9 ]+", "", s.lower().replace("3", "three")).strip()


# ---------------------------------------------------------------- analysis

def score_route(queries, bank):
    """queries: [(cls or None, vec)], bank: [(cls, vec)].
    Class score = max cosine over that class's exemplars."""
    classes = sorted({c for c, _ in bank})
    B = np.array([v for _, v in bank])
    bc = np.array([c for c, _ in bank])
    rows = []
    for cls, q in queries:
        sims = B @ q
        per = sorted(((sims[bc == c].max(), c) for c in classes), reverse=True)
        rows.append((cls, per[0][1], per[0][0], per[0][0] - per[1][0]))
    return rows


def summarize(rows):
    cmd = [r for r in rows if r[0] is not None]
    non = [r for r in rows if r[0] is None]
    acc = np.mean([r[0] == r[1] for r in cmd])
    margin_ok = np.median([r[3] for r in cmd if r[0] == r[1]]) if any(r[0] == r[1] for r in cmd) else 0
    non_max = max(r[2] for r in non)
    # zero false triggers on non-commands: threshold just above the best non-command
    caught0 = np.mean([r[0] == r[1] and r[2] > non_max for r in cmd])
    # allow one false trigger
    non_2nd = sorted((r[2] for r in non), reverse=True)[1]
    caught1 = np.mean([r[0] == r[1] and r[2] > non_2nd for r in cmd])
    # AUC command-vs-non on the top score
    cs = np.array([r[2] for r in cmd]); ns = np.array([r[2] for r in non])
    auc = np.mean(cs[:, None] > ns[None, :])
    return dict(n_cmd=len(cmd), n_non=len(non), top1=acc, med_margin=margin_ok,
                cmd_med=np.median(cs), non_med=np.median(ns), non_max=non_max,
                recall_at_0fp=caught0, recall_at_1fp=caught1, auc=auc)


def report():
    emb = {k: np.array(v) for k, v in load_cache("emb").items()}
    asr_cache = load_cache("asr")
    lat = load_cache("lat")
    clips = clip_list()
    qclips = [c for c in clips if c[0] != "A"]
    lines = []
    P = lines.append

    def table(title, results):
        P(f"\n### {title}\n")
        P("| route | n cmd/non | top-1 | median margin | cmd score med | non score med / max | recall @0 FP | recall @1 FP | AUC |")
        P("|---|---|---|---|---|---|---|---|---|")
        for name, s in results:
            P(f"| {name} | {s['n_cmd']}/{s['n_non']} | {s['top1']:.0%} | {s['med_margin']:.3f} | {s['cmd_med']:.3f} "
              f"| {s['non_med']:.3f} / {s['non_max']:.3f} | {s['recall_at_0fp']:.0%} | {s['recall_at_1fp']:.0%} | {s['auc']:.2f} |")

    def text_bank(pname, key="text"):
        return [(c, emb[f"{key}|{pname}|{p}"]) for c, ps in COMMANDS.items() for p in ps]

    # few-shot audio bank: voice A clips of the spoken command phrasings
    def audio_bank(aname):
        return [(c, emb[f"audio|{aname}|{p}"]) for v, c, u, p in clips if v == "A" and c is not None]

    for subset_name, subset in (("espeak B+C", [c for c in qclips if c[0] in "BC"]),
                                ("Kokoro am_adam", [c for c in qclips if c[0] == "K"])):
        results = []
        for aname in ("plain", "classification"):
            q = [(c, emb[f"audio|{aname}|{p}"]) for v, c, u, p in subset]
            for pname in TEXT_PREFIXES:
                results.append((f"audio[{aname}] -> text[{pname}]", summarize(score_route(q, text_bank(pname)))))
            results.append((f"audio[{aname}] -> audio few-shot (voice A)", summarize(score_route(q, audio_bank(aname)))))
        for pname in TEXT_PREFIXES:
            q = [(c, emb[f"asrtext|{pname}|{asr_cache[p]}"]) for v, c, u, p in subset]
            results.append((f"ASR -> text[{pname}]", summarize(score_route(q, text_bank(pname)))))
        table(subset_name, results)
        # exact match on the transcript
        phrase2cls = {norm(p): c for c, ps in COMMANDS.items() for p in ps}
        cmd = [(c, phrase2cls.get(norm(asr_cache[p]))) for v, c, u, p in subset if c]
        non = [phrase2cls.get(norm(asr_cache[p])) for v, c, u, p in subset if c is None]
        P(f"\nASR exact match ({subset_name}): commands caught {np.mean([a == b for a, b in cmd]):.0%} "
          f"({sum(a == b for a, b in cmd)}/{len(cmd)}), false triggers {sum(x is not None for x in non)}/{len(non)}")

    P("\n### Combined: transcript first, audio embedding as second opinion\n")
    P("Exact match on the transcript against the ears config phrase lists (plus the exemplars above); "
      "on a miss, accept the class only if audio[plain]->text[document] and ASR->text[similarity] agree "
      "and the audio score clears a threshold set at 0 false triggers on the espeak non-commands (in-sample for B+C; Kokoro is held out).\n")
    cfgmap = config_phrases()
    bc = [c for c in qclips if c[0] in "BC"]
    def route_rows(subset, qkey, bank):
        return score_route([(c, emb[qkey(p)]) for v, c, u, p in subset], bank)
    au = lambda p: f"audio|plain|{p}"
    tx = lambda p: f"asrtext|similarity|{asr_cache[p]}"
    ra_bc = route_rows(bc, au, text_bank("document"))
    rt_bc = route_rows(bc, tx, text_bank("similarity"))
    # thresholds: highest non-command score among agreeing pairs on espeak B+C
    agree_non = [(a[2], t[2]) for (v, c, u, p), a, t in zip(bc, ra_bc, rt_bc) if c is None and a[1] == t[1]]
    tha = max([x for x, _ in agree_non] + [0]) + 1e-6
    P(f"Agreement among non-commands (B+C): {len(agree_non)}; audio threshold {tha:.3f}\n")
    for name, subset in (("espeak B+C", bc), ("Kokoro", [c for c in qclips if c[0] == "K"])):
        ra = route_rows(subset, au, text_bank("document"))
        rt = route_rows(subset, tx, text_bank("similarity"))
        ex_ok = cmd_n = rescued = fp_ex = fp_comb = 0
        for (v, c, u, p), a, t in zip(subset, ra, rt):
            ex = cfgmap.get(norm(asr_cache[p]))
            comb = ex or (a[1] if a[1] == t[1] and a[2] > tha else None)
            if c:
                cmd_n += 1
                ex_ok += ex == c
                rescued += (ex != c and comb == c)
            else:
                fp_ex += ex is not None
                fp_comb += comb is not None
        n_non = sum(c is None for _, c, _, _ in subset)
        P(f"- {name}: exact(config) caught {ex_ok}/{cmd_n}, FP {fp_ex}/{n_non}; "
          f"+ audio/ASR-embedding agreement rescues {rescued} more, FP {fp_comb}/{n_non}")

    P("\n### Transcripts that missed (exact match on exemplars only)\n")
    phrase2cls = {norm(p): c for c, ps in COMMANDS.items() for p in ps}
    for v, c, u, p in qclips:
        got = phrase2cls.get(norm(asr_cache[p]))
        if got != c:
            P(f"- {v} `{u}` -> `{asr_cache[p]}` ({got})")

    # per-clip detail for the best pure-audio route, to see what confuses it
    P("\n### Per-clip detail: audio[plain] -> text[classification], espeak B+C + Kokoro\n")
    q = [(c, emb[f"audio|plain|{p}"]) for v, c, u, p in qclips]
    for (v, c, u, p), r in zip(qclips, score_route(q, text_bank("classification"))):
        mark = "ok" if r[0] == r[1] else ("NON" if c is None else "WRONG")
        P(f"- {v} {mark:5} `{u}` -> {r[1]} {r[2]:.3f} (margin {r[3]:.3f})")

    P("\n### Latency (warm, sequential, seconds)\n")
    for k in ("text", "audio", "asr"):
        a = np.array(lat.get(k, []))
        if len(a):
            P(f"- {k}: n={len(a)} median {np.median(a)*1000:.0f} ms, p90 {np.percentile(a, 90)*1000:.0f} ms")
    print("\n".join(lines))


if __name__ == "__main__":
    step = sys.argv[1] if len(sys.argv) > 1 else "all"
    if step in ("clips", "all"):
        make_clips()
    if step in ("embed", "all"):
        run_embed()
    if step in ("asr", "all"):
        run_asr()
    if step in ("report", "all"):
        report()
