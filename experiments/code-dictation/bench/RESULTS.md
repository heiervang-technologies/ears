# Dictated Python: end-to-end benchmark

Does dictating Python through ears and `ears_code.lua` work without fiddling?
**Not yet.** If you speak the README grammar exactly and your words are
typed in perfectly, 73% of utterances do what you meant. Through the ASR it
is 64% with a good voice and 49% with a robotic one. If you speak the way
people talk, it is 16 to 19%, whatever the input. None of the 5 tasks ends
in a correct file in any configuration. An oracle translator, run through the
same plugin, gets 99% and 5/5 correct files. So the harness is sound and the
gap is in the translator and command grammar, not in the plugin's plumbing.

## How it works

- `tasks/<n>/start.py`, `target.py`, `script.txt`: a start file, the file it
  should become, and the utterances that get it there. Each utterance has
  a strict form (README grammar) and, where people would say it
  differently, a natural one (`~`). `~ -` means the natural variant says a
  group of steps in one go, on the group's last step. `@do` is the step's
  ideal effect (keys, or lines as the plugin inserts them). `@line`,
  `@row`, `@cursor`, `@mode`, `@lines` and `===` blocks (`▮` = cursor) are
  hand-written checks.
- **Golden run.** The `@do` effects are replayed in Neovim to get the
  expected buffer, cursor and mode after every step. The hand-written
  checks are then asserted against these states, and so is the target
  file. Any mismatch is printed as `HARNESS:`, and there are none.
- **Resync run** (per-utterance numbers). Before each utterance, the
  buffer, cursor and mode are set to the golden state, so every utterance
  is scored on its own and earlier failures can't spill over. A step
  passes if, afterwards, the text matches token for token (spacing between
  Python tokens is ignored, `* *` is not `**`), the row and mode match, and
  the column matches whenever the line does. "exact" also requires
  byte-identical lines.
- **Chain run** (final file). The same utterances with no resetting, so
  this is what a user would end up with. It is scored exact, after
  `ruff format`, AST-equal, and whether it parses at all.
- `driver.lua` runs inside `nvim --headless -n -u NONE -i NONE` with
  `filetype plugin indent on` (Python's ftplugin/indent: `sw=4`, `O`/`o`
  indent as stock Vim does). It loads the real `ears_code.lua` and plays
  each utterance the way the ears daemon would. If the whole utterance is
  an ears command (from `~/.config/ears/config.toml` `[commands]`: "new
  line", "enter", "over", "scratch that", "back N", ...), it sends a
  `VoiceCommand`. Otherwise it sends `SpeechStarted` and then
  `SegmentCompleted`. Keys the plugin feeds (mode switches, `ciw`, `<Esc>`)
  go through Neovim's real input loop.
- Audio mode: Kokoro `af_heart` (shared service, so throttled to 1
  request/s, and the run stops if the pod restarts) and local `espeak-ng
  en-us+m3`, resampled to 16 kHz mono. Each clip is transcribed by
  Qwen3-ASR-1.7B at the time of the step, with the current buffer's names
  as context, exactly as `dictate.py` does (retrying without context if
  nothing is heard). Audio and transcripts are cached on KC3 at
  `/run/media/me/storage/builds/ears/code-dictation-bench/` (15 MB).

Plugin change (test hook only, behaviour unchanged when unset):
`EARS_CODE_HEADLESS=1` skips the ears sockets and focus checks and holds a
stand-in claim. `EARS_CODE_TRANSLATOR` replaces the `dictate.py --serve`
command.

## Running it

```bash
cd experiments/code-dictation
python3 bench/run.py                       # text input: 30 s
python3 bench/run.py --audio               # + both voices (needs ASR, TTS, KC3)
python3 bench/run.py --tasks 2-add,5-retry --variants strict
python3 bench/run.py --translator 'python3 bench/backends/oracle.py' --label oracle
```

Results go to `bench/results/<label>.md` (summary and every failed
utterance) and `.json` (every step: heard, line, mode, cursor, latencies,
plugin messages).

**Plugging in a backend.** `--translator CMD` (or the `EARS_CODE_TRANSLATOR`
env var) is any shell command that speaks `dictate.py --serve`'s JSON-lines
protocol on stdin and stdout:

```
in:  {"id": 3, "lang": "py", "text": "<what ears heard>", "scope": [names...], "mode": "insert"|"normal"|"visual"}
out: {"id": 3, "lines": [{"code": "x = 1", "shift": 0}]}   # insert mode: lines to insert
     {"id": 3, "keys": "<Esc>"}                             # any mode: keys to feed (null = not a command)
```

`EARS_BENCH_TASK` names the task being played. `bench/backends/oracle.py`
is a worked example. A LoRA or ASR-direct backend needs nothing else. A
backend that wants the audio rather than the transcript would also need
the clip path, which the protocol does not carry yet; adding a `wav` field
in `driver.lua` → `on_event` would be the place.

## Baseline (dictate.py + spoken.py at 0e58741, 2026-10-22)

Utterances passed (resync run), out of 78 strict and 64 natural; final
files AST-equal out of 5 (chain run):

| input | strict | natural | final AST (strict / natural) | final parses |
|---|---|---|---|---|
| text (translator only) | **57/78 (73%)** | 12/64 (19%) | 0/5 / 0/5 | 1/5 / 3/5 |
| Kokoro af_heart → ASR | 50/78 (64%) | 11/64 (17%) | 0/5 / 0/5 | 0/5 / 3/5 |
| espeak-ng → ASR | 38/78 (49%) | 10/64 (16%) | 0/5 / 0/5 | 0/5 / 3/5 |
| text, **oracle** translator | 77/78 (99%) | 62/64 (97%) | 5/5 / 5/5 (ruff 5/5, exact 2/5) | 5/5 |
| af_heart → ASR, oracle | 68/78 (87%) | 53/64 (83%) | 1/5 / 2/5 | 4/5 / 5/5 |
| espeak → ASR, oracle | 50/78 (64%) | 39/64 (61%) | 0/5 / 0/5 | 4/5 / 5/5 |

The oracle rows with audio are the ceiling the ASR leaves for a translator
that knows the answer but needs the right words. It looks up the heard
text after normalizing case, punctuation and `_`, so "Deaf main" misses.
A translator that tolerates mishearings could beat this. With a good
voice, roughly one utterance in eight is lost to the ASR alone. A single
miss in a 12 to 20 step task sinks the final file, which is why only 1
or 2 of 5 files come out right. Of the utterances af_heart heard word for
word, the oracle passed 118/121 (the baseline translator: 61/122). Run
with `--asr-cache /tmp/...`, which reads the cached clips and writes
nothing on KC3. Results are in `results/oracle.md`.

Per task, text strict: argparse CLI 7/14, add methods 10/12, refactor
18/20, dataclass 12/16, in-place fixes 10/16. Navigation and normal-mode
editing ("go to def add", "open below", "indent twice", "delete line",
"change string") almost always work when phrased strictly. Code lines are
where it fails.

**Latency** (median / p90): ASR 110 / 197 ms per utterance with context
(n=539, max 483). Translation plus applying it in Neovim: 2 / 3 ms. The
first utterance of a session costs 51 ms (translator start-up). Latency
is not the problem.

**ASR**: word error rate is 8% for af_heart and 32% for espeak. 122/142
af_heart utterances were heard word for word (normalized), and only half
of those (61/122) passed, so the translator is the bottleneck even with
perfect hearing.

## Failure analysis

Counts are failed utterances in the resync run across all six
configurations (text/af_heart/espeak × strict/natural), 248 in total. The
auto-tag is the first guess in `run.py`. The breakdown below is the manual
triage of the text runs plus the ASR samples.

### 1. Translator gaps (`spoken.py`): the largest group, 111 + most of the 48 "asr+text-fails-too"

Strict-grammar failures, text input:

| said | got | cause |
|---|---|---|
| def parse args | `def argparse(args)():` | fuzzy scope match: "parse" → `argparse` (ratio 0.77 ≥ 0.75), then "args" becomes a call |
| parser equals argparse dot pascal argument parser of description equals string … | `parse_args = argparse.Argument(parse_args(description = "…"))` | "parser" fuzzy → `parse_args`; `pascal` swallows the following words |
| raise pascal key error of name | `raise KeyErrorOf(name)` | `pascal`/`snake` casing runs until a non-word token, swallowing "of" |
| parser dot add argument of string path (with `add_argument` in scope) | `parser(add_argument("path"))` | multi-word fuzzy match "dot add argument" → `add_argument` eats the spoken "dot" |
| … of quote dash dash top quote comma type equals int … | `("-- top", type(= int(, …` | `--` inside a string gets a space; a callable (`int`) followed by a comma opens a call |
| print of f string reading brace args dot path | `f"reading {args}. path"` | `brace NAME` takes one token; no `{expr}` |
| log dot info of f string brace len of unpaid close unpaid orders | `log.info(f"{len} of unpaid") unpaid(orders)` | same: no expression inside f-string braces |
| dedent if dunder name equals equals string dunder main | `if __name__ == "dunder main":` | casing words not applied inside strings |
| main of | `__main__()` (sometimes `main()`) | `main` and `__main__` both normalize to `main` in the scope table; which wins depends on Lua's hash order → **nondeterministic** (passed in one run, failed in the next) |
| dedent def unpaid total taking orders | `def paid(total)(Order):` | fuzzy: "unpaid" → `paid`, "orders" → `Order` |
| unpaid equals brackets o dot price … | `paid = [...]` | same |
| time dot sleep of two star star attempt | `time.sleep(2 * * attempt)` | no `**` |
| f string giving up on brace url (line start) | `def "giving up on brace url"():` | a leading "f" is always taken as a misheard "def" |

The common thread is that fuzzy matching against scope is too eager and
fires on words that are new names being defined (`unpaid`, `parser`,
`main`). It should not fire on a name being assigned or defined, it needs
a higher threshold for short words, and it must never cross a spoken
symbol.

Natural phrasing (text input, 52 failures) shows how far the grammar is
from how people talk. Filler words ("define **a function called** parse
args", "return **the** sum of", "return **an** empty dict", "**a list
of**"), no type annotation without "colon" ("id int"), "defaults to", "to
the power of", "in square brackets", "call main", "if name equals main"
for the `__main__` idiom, "two blank lines" → `2 blank_lines`, "blank
line" → `blank_line`, "def total self" → `def total(self)():` (no "taking"
→ the args become a call inside the name).

### 2. Command gaps (normal mode): 35 command-gap + 19 target-not-found

- Natural motions are not in the closed list: "next line", "open a line
  below", "new line above", "go to the last line", "indent two lines",
  "normal mode" (it is an insert-mode command only; in normal mode it is
  "not a command").
- Compound edits are missing: "rename load to load config", "change
  amount to price", "change three to five", "change the string to …",
  "delete the print line". "change X to Y" parses as `change` with target
  `x_to_y` and fails with "not found". "change timeout to ten" deletes
  `timeout` and stops.
- Targets: filler words become part of the pattern ("go to **the** add
  method" → `\<the_\?add_\?method\>`). Multi-word targets only match as
  one identifier ("go to return data", "go to giving up" fail because the
  pattern is `return_\?data`). **Numbers can't be targets**: in "go to
  three" / "go to five", `take_count` treats the number as a repeat count.

### 3. Plugin gaps (`ears_code.lua`)

- **Dictation never goes in at the cursor.** In insert mode, every
  utterance becomes a whole new line (it replaces the current line if
  blank, otherwise goes below it). So the README flow "change parse args,
  then say the new name" puts `load(config)` on a new line under `def
  (path):`, and "change word" + "five" gives a line `5` instead of
  `retries=5`. The README's "after parse args, then of …" has the same
  problem. This breaks every in-place edit in tasks 3 and 5. A fix would
  be to insert inline when the cursor is not at the end of a non-blank
  line, or after a `c`/"after X"; I've only reported this, not changed it.
- **"new line" on a blank line does nothing.** ears sends it as a
  `VoiceCommand`, and `insert_lines({{code=""}})` replaces the blank
  current line with itself. After "open below", "new line" leaves you
  where you were, and "double new line" on a blank line gives one line,
  not two. This is the only failure the oracle translator can't fix,
  because ears commands never reach the translator.
- (Harness note, not a plugin bug.) `O` indentation comes from Vim's
  indent rules (4 here, above `return data` under a `try` body). The
  strict script says "except …" without "dedent" because that's what the
  screen shows. With NvChad/treesitter indent this may differ.

### 4. ASR: 29 failures pure ASR (text passed, audio failed); 48 where ASR and translator both failed

- `dedent` is barely a word to the model: "Dead and", "Debent", "Bed and
  theft", "There's no", "Deadendel". 13 of 14 "dedent …" utterances failed
  in audio. "dedent def" was never heard right ("Dead and deaf"). af_heart
  got "dedent" right twice, both times before something other than "def".
  "outdent" or "unindent" (both already accepted in `LAYOUT`) are likely
  safer.
- `def`: heard as "Deaf" (handled by `HEADS`), but also "Death", "The
  parsons", "Neffad" (espeak).
- Symbols as words: "comma" → "common", "id int" → "It ain't" / "I
  didn't", "brace url" → "raise URL", "brace len" → "bracelet", "at sign"
  → "At signed", "o in" → "loaded", "two" → "to" ("sleep to to the power").
- **Context regurgitation**: 4 espeak utterances (plus one af_heart in a
  smoke run) came back as the context word list instead of the speech,
  e.g. "dedent del self dot items index name" → `self name items def
  count Inventory add inv get remove apple if class KeyError not __init__
  raise`. The translator then turns that into a 15-deep call chain and
  inserts it. Output that is mostly context names and has no sentence
  punctuation could be detected and retried without context.
- **Language drift** (espeak only): "delete the print line" → "Verlees de
  printlijn" (Dutch), "go to giving" → "rồi sẽ gây viêm" (Vietnamese), "at
  dataclass" → "Eerste klas". Forcing English would remove these.
- What works: context makes identifiers come back as code. "parse args" →
  `parse_args`, "add argument" → `add_argument`, "self dot items" →
  `self.items`. So the ASR already does part of the translator's job, and
  the translator needs to accept code-shaped input too (it mostly does).
- espeak is much harder to hear than Kokoro (WER 32% vs 8%). A real voice
  is probably closer to Kokoro, but these voices don't stand in for
  accents, noise or microphone distance.

### 5. ears commands: 3

Only "new line" (see plugin gaps). Note that with the user's config,
"enter", "over", "new line" and "scratch that" never reach the
translator, so any backend has to live with how the plugin handles them.

## What would move the numbers most

1. Inline insertion at the cursor (plugin). This unblocks every in-place
   edit.
2. Tame fuzzy scope matching: no matching for names being defined or
   assigned, no matching across spoken symbols, and deterministic
   tie-breaking.
3. A natural-language layer for commands and code (filler words, "X to
   Y" edits, "next line", "two blank lines", "call X", "if name equals
   main"). This is where an LLM/LoRA backend should win most. Plug it in
   with `--translator` and compare against the 12/64 natural baseline.
4. Hardening at the ASR boundary: retry on regurgitated context, force
   English, and pick spoken forms the model hears ("outdent", not
   "dedent").

## Not done

- No real human voice and no partials (`Partial` events and ghost text
  are not exercised; only final segments are).
