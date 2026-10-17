"""Corpus -> JSONL training records for spoken code: {context, code, spoken, style}.

Each single-line statement, block head and decorator of a local Python file
becomes one record per style (verbalize.py). Terse and explicit records carry
`verified`: spoken.py turns them back into the same code (same AST).

Writes, under OUT (default: the KC3 build dir):
  records.jsonl  {context, code, indent, spoken, style, kind, verified, source, path, lineno}
  scopes.jsonl   {path, scope}: the names a dictation tool would have in scope
  stats.json

Usage: python make_dataset.py [--files N] [--out DIR] [--budget-mb MB]
"""
import argparse
import ast
import collections
import json
import os
import sys

import corpus
from roundtrip import SKIP, back, same
from verbalize import lines_of, scope_of, verbalize

OUT = "/run/media/me/storage/builds/ears/code-dictation-verbalizer/dataset"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--files", type=int, default=300, help="files per source")
    ap.add_argument("--context", type=int, default=4, help="preceding lines as context")
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--budget-mb", type=int, default=1500)
    ap.add_argument("--seed", type=int, default=0)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    stats = collections.Counter()
    unsayable = collections.Counter()
    records = open(os.path.join(a.out, "records.jsonl"), "w")
    scopes = open(os.path.join(a.out, "scopes.jsonl"), "w")
    for source, paths in corpus.files(a.files, a.seed + 100).items():
        for path in paths:
            try:
                src = open(path, encoding="utf-8").read()
                tree = ast.parse(src)
            except (SyntaxError, UnicodeDecodeError, ValueError):
                stats["files unparsable"] += 1
                continue
            stats["files"] += 1
            scope = scope_of(tree)
            scopes.write(json.dumps({"path": path, "scope": scope}) + "\n")
            phys = src.splitlines()
            for line in lines_of(src):
                if line.kind.startswith(SKIP):
                    stats["lines skipped (" + line.kind.split("-")[0] + ")"] += 1
                    continue
                stats["lines"] += 1
                said, errors = verbalize(line, src, a.seed, scope)
                for style, err in errors:
                    unsayable[f"{style}: {err}"] += 1
                if not said:
                    continue
                text = phys[line.lineno - 1]
                context = [l for l in phys[max(0, line.lineno - 1 - a.context):line.lineno - 1]]
                for style, spoken_text in said:
                    rec = {"context": "\n".join(context), "code": line.code,
                           "indent": text[:len(text) - len(text.lstrip())],
                           "spoken": spoken_text, "style": style, "kind": line.kind,
                           "source": source, "path": path, "lineno": line.lineno}
                    if style in ("terse", "explicit"):
                        rec["verified"] = same(line.code, back(spoken_text, scope))
                        stats[f"verified {style}"] += rec["verified"]
                    stats[f"records {style}"] += 1
                    records.write(json.dumps(rec) + "\n")
            if records.tell() > a.budget_mb * 1_000_000:
                print("budget reached, stopping", file=sys.stderr)
                break
    records.close()
    scopes.close()
    stats = dict(stats)
    stats["unsayable"] = dict(unsayable.most_common())
    stats["bytes"] = os.path.getsize(os.path.join(a.out, "records.jsonl"))
    json.dump(stats, open(os.path.join(a.out, "stats.json"), "w"), indent=1)
    print(json.dumps(stats, indent=1))


if __name__ == "__main__":
    main()
