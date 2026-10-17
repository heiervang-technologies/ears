# Verbalizer: real Python -> what a person would say

Synthetic training data for a possible code-dictation LoRA on the ASR decoder:
real code goes in, several spoken variants come out. It runs in the opposite
direction to `spoken.py`, and the round trip through `spoken.py` measures how
much real code the rules cover.

## Files

- `verbalize.py`: a line with its file -> `[(style, spoken)]`. It uses `ast` for
  structure and `tokenize` to strip comments. Styles:
  - `terse`: spoken.py's dialect. It leaves out what goes without saying (the
    colon after a block head, closing brackets at the end of the line, quotes,
    comma spacing, a leading `_` when scope restores it).
    `x equals foo of a comma string b`.
  - `explicit`: every symbol said. `foo open paren a close paren colon`.
  - `natural`: English with seeded variety. `set x to call foo with a and b`,
    `add one to count`, `define method run taking x` (self left out), `catch
    value error as e`.
  - `noself`: natural, with `self dot` dropped inside methods. Only the
    context can restore it.
  Where spoken.py had no form, terse uses the phrase a person would most
  likely try (`at property`, `class foo of base`, `def f taking x equals
  none returns int`, `star star`, `less than or equal`).
- `roundtrip.py`: corpus -> terse/explicit -> `spoken.code_lines('py', ...)`
  with the file's names as scope. It compares ASTs: *exact*, and *lenient*
  (case and spacing inside strings forgiven, since spoken words carry
  neither). For every failing line it names the smallest sub-expression that
  also fails on its own.
- `make_dataset.py`: corpus -> `records.jsonl` `{context, code, indent, spoken,
  style, kind, verified, source, path, lineno}` plus `scopes.jsonl` (the
  names per file). `verified` = spoken.py gives the line back.
- `audio_sample.py`: TTS sample -> Qwen3-ASR -> WER and end-to-end code
  accuracy.
- `corpus.py`: local files only. Sources are the stdlib, site-packages (at
  most 40 files per package) and `~/ht` (at most 40 per repo). Tests, vendored
  code and venvs are excluded, and files are deduplicated by content.
- `examples.jsonl`: 40 records (10 lines x 4 styles). `results/*.txt` holds
  the full round-trip and ASR reports.

Bulk output (not in git) lives in
`/run/media/me/storage/builds/ears/code-dictation-verbalizer/` (204 MB):
`dataset/` (177 MB, 406,466 records from 900 files and 130,608 lines,
about 15 minutes on one core). It was generated before the last change to
the natural style, which now quotes strings that contain "comma"; KC3 fell
below 30 GB free, so I did not regenerate it. and `audio/` (229 clips,
34 MB) plus `asr.jsonl`.

## Coverage

What can't be said at all (terse unsayable 2.6% of lines): non-decimal and
underscored number literals (`0xFF`, `1_000`), f-string format specs and
conversions (`{x!r}`, `{x:>8}`), control characters, non-ASCII and
whitespace-only runs in strings, the name `_` alone, t-strings and `match`.
Lines the dataset skips: statements spanning several physical lines (6.2% of
statements) and one-line compound statements (`if x: return`).

Constructs covered: assignment (plain, annotated, augmented, chained,
unpacking), calls (nested, kwargs, `*args`/`**kw`, generator argument),
attributes, subscripts and slices, every binary, unary, boolean and comparison
operator, ternaries, lambda, walrus, await/yield, list/dict/set/tuple
literals, comprehensions with conditions, strings, f-strings with simple
fields, bytes, and floats. Statement heads covered: def, async def, class,
if, elif, else, for, while, with, try, except, finally, return, raise from,
assert, del, global/nonlocal, import, relative from-import, decorators.

## Round trip: how much real code spoken.py takes back

5,750 sampled lines from 176 files (60 per source, 40 lines each). Rates are
shares of all sampled lines, unsayable ones included.

| | terse exact | terse lenient | explicit lenient |
|---|---|---|---|
| **baseline**: spoken.py as it was, first-cut verbalizer | 47.4% | 51.9% | 53.0% |
| after the fixes below | 83.8% | **92.5%** | 91.4% |
| same, held-out sample (`--seed 7`, 5,620 lines) | 84.4% | 92.7% | 91.7% |

By source (final, terse lenient): stdlib 95.4%, site-packages 93.5%, ~/ht 90.0%.

By statement, terse lenient (baseline -> final):

| statement | n | baseline | final | | statement | n | baseline | final |
|---|---|---|---|---|---|---|---|---|
| assign | 1573 | 45.5% | 87.8% | | class | 137 | 11.7% | 94.9% |
| call | 736 | 39.5% | 84.6% | | except | 93 | 52.7% | 97.8% |
| def | 593 | 30.4% | 99.2% | | raise | 93 | 28.0% | 69.9% |
| if | 584 | 65.4% | 95.7% | | annassign | 80 | 7.5% | 100% |
| return | 579 | 62.2% | 94.3% | | decorator | 55 | 0% | 98.2% |
| from-import | 197 | 42.1% | 99.5% | | async def | 21 | 0% | 95.2% |
| for | 149 | 77.9% | 94.6% | | with | 50 | 66.0% | 100% |

By construct on the line, terse lenient (baseline -> final): call 45 -> 89%,
attribute 40 -> 91%, string 44 -> 76% (exact only 34%), f-string 26 -> 36%,
kwarg 32 -> 87%, param default 1 -> 99%, annotation 0 -> 98%, float 1 -> 86%,
int > 12 29 -> 63%, `|` 4 -> 98%, `<=`/`>=` 10/0 -> 90/89%, `//` 0 -> 77%.

### Top failure categories of the original rules (baseline, share of all lines)

1. **Fuzzy name matching swallowed neighbouring words (14.5%).** `codecs dot
   charmap encode` became `codecs(charmap_encode(...))`, because
   "dotcharmapencode" fuzzy-matched `charmap_encode`. The same thing ate
   `comma` (`register_converter("date convert_date")`) and `taking` (`def
   f(text)():`).
2. **Names containing a word that means code (5.3%):** `selective_len`,
   `is_file`, `TRY_NATIVE` (a line-start "try"), `type_alias`, and builtins
   used as values (`type`, `any`, `max` before a comma open a call).
3. **Block heads with more than bare names (about 7% together):** default
   parameters, annotations, `*args`, return annotations, class bases,
   decorators, `async`, relative imports and dotted imports of multi-word
   names all failed.
4. **Case and underscore collisions in scope:** `Event` and `event`,
   `_inner` and `inner`, `HTTPServer` (not seen as a multi-word name).
5. Numbers: teens (`thirteen` was a name), floats, and `print of` with no
   arguments at the end of a line (it gave `print`, not `print()`).
6. Operators with no spoken form: `**`, `//`, `<=`, `>=`, `&`, `^`, `<<`,
   `>>`, and the augmented forms `//=`, `*=` and others.
7. Strings: case and spacing are lost (`"I/O"` comes back as `"i/ o"`), and so
   are content words that act as commands (`close`, `closed`, `end`, numbers).

### Fixes made to spoken.py (small, each local)

The `py-heard.txt` and README examples give the same output as before. The
one difference is the already-broken "f string ..." at the start of a line.

- `lex`: names of up to 6 words. A fuzzy multi-word match needs exactly as
  many words as the name has, and never contains a symbol word. A single word
  fuzzy-matches only one-word names. HTTPServer-style names count as
  multi-word.
- Casing words (`snake`, `pascal`, `constant`, `camel`, `dunder`) now take the
  longest run of words that names something in scope ("snake is file",
  "pascal sax exception" -> `SAXException`), matching case-insensitively. They
  stop at "of" and similar.
- `underscore X` -> `_X`.
- Python-only operators (`PY_SYMBOLS`, so `bash_line` is unchanged): `** // <=
  >= & ^ << >> ->`, augmented forms, `bang`, `question mark`.
- Teens, decimal points (`sixty point five`, and ASR's `0.5`), `bytes string`.
- Built-ins open a call only before a value (`range ten`, `sum of x`, not
  `type comma`). `print of` at the end of a line gives `print()`.
- In conditions, `=` -> `==` no longer applies inside brackets (keyword
  arguments).
- Heads: `def NAME taking PARAMS returns T` (defaults, annotations, `*`,
  `**`), `class NAME of BASES`, `at NAME` (decorators), `async ...`,
  `global`/`nonlocal`. Import names are always names (`from typing import any
  comma type alias`), and `from dot x` handles relative imports.

The verbalizer adds the matching habit: it says a casing word or "underscore"
only when a name is ambiguous in scope or holds a word that means code.

**Caveat:** the final 92.5% is the rules and the verbalizer agreeing on a
dialect that I tuned on one sample. The held-out sample confirms this isn't
overfit to particular files, but it does not show that people speak this way.
The baseline 52% is the honest measure of the rules as they stood.

### What still fails (final, 4.9% of lines)

String content (case, spacing, words like close/end/numbers, about 3.4%),
f-string fields that aren't a single name (`{self.x}`), bytes literals in some
forms, names holding "closed"/"non" (MISHEARD maps them to close/none), names
holding layout words (`NEWLINE`, `indent` split the line before any rule
runs), `filter` vs `filter_` (leading/trailing underscore collisions), and a
leading `_` on class names.

## ASR on spoken code (audio sample)

100 dataset lines (40 terse, 30 explicit, 30 natural; terse and explicit lines
all verified), each run through Qwen3-ASR-1.7B with no context and again with
the file's names as context.

**Kokoro problem:** the `kokoro-tts` pod (2 GiB limit) was OOMKilled every
10-15 sequential requests. I stopped using Kokoro after 29 clips (15 lines,
af_heart and bm_george); the full 100 lines use espeak-ng (default voice, and
en-us+f3 at 135 wpm). 229 clips in total.

| no context | n | WER | all words right | code exact (terse+explicit) |
|---|---|---|---|---|
| Kokoro af_heart | 15 | 17.9% | 47% | 60% |
| Kokoro bm_george | 14 | 16.9% | 50% | 71% |
| espeak default | 100 | 29.2% | 19% | 26% |
| espeak en-us+f3 | 100 | 42.7% | 7% | 9% |
| style terse / explicit / natural | | 32.1 / 38.7 / 30.9% | | 34 / 10 / - |

With scope names as context, WER gets **worse** (33.8% -> 39.3% overall;
Kokoro 17-18% -> 20-23%). Code exact drops from 25% to 20%. Median latency is
178 ms.

What the ASR does with spoken code:
- When the transcript is word-perfect, spoken.py gives the exact code 100% of
  the time (31/31), so the remaining loss is in hearing the words.
- With context, the ASR starts writing code instead of words: `root_definition`,
  `Image.snake.register.open_of_...`, `SnakeCloseError`. Transcripts containing
  `, _ ( = [` go from 30 to 59 of 229. spoken.py then mis-parses them
  (`root_definition` is no longer the words "root definition"). Even without
  context, "comma" often comes back as `,`, which spoken.py's punctuation
  stripping deletes (`except (A, B)` became `except A(B)`).
- Command words get misheard: `dot` -> `not` (56, mostly espeak), `except` ->
  `accept`, `elif` -> `eliphance`/`a lift`, `index` -> `in`/`indexed`, `self` ->
  `if`/`else`/`elf`, `dict` -> `vic`, `io` -> `I/O`.
- espeak is much harder to hear than Kokoro or a real voice, so these numbers
  are pessimistic. Even so, decoder-side training aimed at the spoken-code
  vocabulary (or emitting code directly) would target exactly these errors.

## Scaling to a LoRA dataset

- Text: the generator runs on one core at about 9k lines a minute, so 1M lines
  takes about 2 hours and 1.4 GB of JSONL. Use `--budget-mb`. The corpus is
  local only; ~/ht and site-packages hold maybe 3-5M usable lines after dedup.
  Filter on `verified` for the terse/explicit targets.
- Audio is the bottleneck. Kokoro in its current pod can't do bulk work: it
  needs a higher memory limit or a restart policy, and that is a k8s change I
  didn't make. espeak is free but unrealistic. Mixing several TTS voices, plus
  speed and noise augmentation, plus a few hundred real recordings for
  validation, is the minimum. At roughly 4 s per clip, 100k clips is about
  15 GB of 16 kHz WAV (FLAC halves that). That exceeds the current KC3
  headroom (25 GB free and falling while I worked).
- Targets: decide whether the LoRA emits the terse words (keeping spoken.py as
  stage 2, which works perfectly on word-perfect input) or code directly. The
  records support both (`spoken` and `code`, plus `context`).
- The natural and noself styles have no round trip. They need a sample review
  by a person, or a learned inverse, before anyone trusts them.

## Effort for another language

The parts that carry over: corpus handling, record format, round-trip harness,
audio and ASR scoring, the casing and underscore disambiguation, and the
string and number speaking (`text_words`, `number_words`). The per-language
parts:

- A parser. Python has `ast` built in. Others need tree-sitter, which isn't
  installed for Python here (Neovim's parsers could be driven headless), or the
  language's own tooling (`bash -n` doesn't give an AST; shfmt or tree-sitter
  do).
- The verbalizer's node methods: about 500 of the 1,050 lines here.
- The inverse rules (spoken.py's `python_line` equivalent), if they stay rule
  based. `bash_line` exists but is much thinner than the Python path.

Estimate for a mid-size language (JS/TS, Rust, Go, bash): **2-3 days** for the
verbalizer plus round-trip baseline, and **3-5 more days** to bring rule-based
inverse coverage to about 90%. If a LoRA learns the inverse, only the first
part is needed, plus perhaps a day per language to review the natural style.
Languages with heavy symbol syntax (Rust lifetimes and generics, shell quoting)
sit at the upper end.
