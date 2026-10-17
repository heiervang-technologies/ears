"""Transcribe a WAV with names in scope as context, then translate to code.

Usage: python dictate.py LANG WAV [SCOPE_FILE]
LANG is py, sh or text. Prints JSON: {"heard": ..., "code": ...}.
"""
import json
import sys

from probe import asr
from spoken import bash_line, python_line

lang, wav = sys.argv[1], sys.argv[2]
scope = open(sys.argv[3]).read().split() if len(sys.argv) > 3 else []

heard, _ = asr(wav, system=" ".join(scope[:300]) or None)
if heard.startswith("RAW:"):  # context made it hear nothing; try without
    heard, _ = asr(wav)
heard = "" if heard.startswith("RAW:") else heard

translate = {"py": python_line, "sh": bash_line}.get(lang)
code = translate(heard, scope) if translate else heard
print(json.dumps({"heard": heard, "code": code}))
