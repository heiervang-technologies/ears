#!/usr/bin/env python3
"""Regenerate fixtures/*.cands.json: load each fixture page in a hover started with
--remote-debugging-port PORT and save what the page collector sees.

  python3 tests/capture.py [PORT]     (use a throwaway headless instance, see README)
"""
import json, sys
from pathlib import Path

from hp import hp

FIX = Path(__file__).resolve().parent / "fixtures"


def main(port):
    b = hp.Bidi.connect(port)
    try:
        ctx = b.call("browsingContext.create", {"type": "tab", "background": True})["context"]
        for page in sorted(FIX.glob("*.html")):
            b.call("browsingContext.navigate", {"context": ctx, "url": page.as_uri(), "wait": "complete"})
            res = b.run(ctx, hp.COLLECT_JS)
            out = page.with_suffix(".cands.json")
            out.write_text(json.dumps(res["cands"], indent=0, ensure_ascii=False) + "\n")
            print(f"{out.name}: {len(res['cands'])} candidates")
        b.call("browsingContext.close", {"context": ctx})
    finally:
        b.close()


if __name__ == "__main__":
    main(int(sys.argv[1]) if len(sys.argv) > 1 else 9333)
