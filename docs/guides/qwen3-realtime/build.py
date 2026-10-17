#!/usr/bin/env python3
"""Inline reveal.js, haios.css and the mascot into one self-contained guide.html."""
import pathlib, sys
here = pathlib.Path(__file__).parent
reveal = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/revealpkg/package")
ds = pathlib.Path.home() / "ht/design-system"
src = (here / "src.html").read_text()
parts = {
    "/*@@REVEAL_CSS@@*/": (reveal / "dist/reveal.css").read_text(),
    "/*@@HAIOS_CSS@@*/": (ds / "dist/haios.css").read_text(),
    "/*@@REVEAL_JS@@*/": (reveal / "dist/reveal.js").read_text(),
    "@@MASCOT@@": (ds / "assets/states/listening.svg").read_text(),
}
for k, v in parts.items():
    assert k in src, k
    src = src.replace(k, v)
(here / "guide.html").write_text(src)
print(here / "guide.html", len(src))
