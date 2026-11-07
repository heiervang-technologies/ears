#!/usr/bin/env python3
"""Fetch LibriSpeech test-clean and test-other (openslr.org, CC BY 4.0) and add
a fixed, seeded subset of each to the manifest. Run on the rig (fast network).

usage: fetch_librispeech.py DATA_DIR [--per-set 300]
"""
import argparse, glob, json, os, random, subprocess, tarfile, urllib.request

URL = "https://www.openslr.org/resources/12/{}.tar.gz"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("data")
    ap.add_argument("--per-set", type=int, default=300)
    a = ap.parse_args()
    raw = os.path.join(a.data, "librispeech")
    os.makedirs(raw, exist_ok=True)
    rows = []
    for name, tag in (("test-clean", "ls-clean"), ("test-other", "ls-other")):
        tgz = os.path.join(raw, name + ".tar.gz")
        if not os.path.exists(os.path.join(raw, "LibriSpeech", name)):
            if not os.path.exists(tgz):
                urllib.request.urlretrieve(URL.format(name), tgz)
            with tarfile.open(tgz) as t:
                t.extractall(raw, filter="data")
        texts = {}
        for tf in glob.glob(os.path.join(raw, "LibriSpeech", name, "*", "*", "*.trans.txt")):
            for line in open(tf):
                id_, text = line.rstrip("\n").split(" ", 1)
                texts[id_] = text
        ids = sorted(texts)
        random.Random(12).shuffle(ids)
        for id_ in sorted(ids[:a.per_set]):
            spk, chap, _ = id_.split("-")
            flac = os.path.join(raw, "LibriSpeech", name, spk, chap, id_ + ".flac")
            rel = os.path.join("ls", tag, id_ + ".pcm")
            out = os.path.join(a.data, rel)
            os.makedirs(os.path.dirname(out), exist_ok=True)
            if not os.path.exists(out):
                subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", flac, "-ac", "1",
                                "-ar", "16000", "-f", "s16le", out], check=True)
            rows.append({"id": id_, "pcm": rel, "text": texts[id_], "set": tag, "latency": False})
    with open(os.path.join(a.data, "manifest.jsonl"), "a") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    print(f"{len(rows)} LibriSpeech clips added")


if __name__ == "__main__":
    main()
