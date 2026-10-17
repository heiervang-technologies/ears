"""Local Python files to verbalize: the stdlib, installed packages and the
~/ht repositories. Nothing is downloaded. Copies of the same file (the many
checkouts of one repo) count once."""
import glob
import hashlib
import os
import random

SKIP_DIRS = {".git", ".venv", "venv", "site-packages", "node_modules", "__pycache__",
             "build", "dist", "test", "tests", ".tox", "vendor", "third_party"}


def walk(root, cap_per_top=None):
    """.py files under root, at most cap_per_top from each top-level dir."""
    per_top = {}
    for dirpath, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS and not d.startswith("."))
        top = os.path.relpath(dirpath, root).split(os.sep)[0]
        for f in sorted(files):
            if f.endswith(".py"):
                per_top.setdefault(top, []).append(os.path.join(dirpath, f))
    out = []
    rng = random.Random(1)
    for top, paths in sorted(per_top.items()):
        rng.shuffle(paths)
        out += paths[:cap_per_top] if cap_per_top else paths
    return out


def sources():
    stdlib = glob.glob("/usr/lib/python3*/")[0]
    site = os.path.join(stdlib, "site-packages")
    std = [p for p in walk(stdlib) if "/site-packages/" not in p and "/idlelib/" not in p]
    return {"stdlib": std, "site-packages": walk(site, 40),
            "ht": walk(os.path.expanduser("~/ht"), 40)}


def files(per_source, seed=0, max_bytes=200_000):
    """{source: [path]}, per_source files from each, deduplicated by content."""
    rng, seen, out = random.Random(seed), set(), {}
    for name, paths in sources().items():
        paths = sorted(paths)
        rng.shuffle(paths)
        picked = []
        for p in paths:
            try:
                if os.path.getsize(p) > max_bytes:
                    continue
                digest = hashlib.sha1(open(p, "rb").read()).hexdigest()
            except OSError:
                continue
            if digest in seen:
                continue
            seen.add(digest)
            picked.append(p)
            if len(picked) >= per_source:
                break
        out[name] = picked
    return out
