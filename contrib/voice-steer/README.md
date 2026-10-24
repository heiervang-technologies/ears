# voice-steer: steer the hover browser by voice

`hover-page` clicks, focuses, types into and scrolls the page you are looking at in
hover, finding elements by what they are called rather than where they are.
`hover-page.toml` is a [voice-cmd](../../README.md) plugin that maps spoken phrases
to it. With ears' `steer = ["voice-cmd", "handle"]`, saying *"click subscriptions"*
or *"search for rust tutorials"* while hover is under the pointer acts on the page.

It is deliberately not an agent: no model in the loop, one WebDriver BiDi round trip
to read the page, fuzzy string matching, one more to act. A command takes about
50-70 ms on ordinary pages and around 100 ms on very large ones.

## Install

```bash
install -m 755 hover-page ~/.local/bin/hover-page     # voice-cmd puts ~/.local/bin on PATH
install -m 644 hover-page.toml ~/.config/voice-commands/hover-page.toml
```

hover must run with `--remote-debugging-port 9333 --remote-allow-system-access`
(its desktop file does). `hover-page` is Python 3 standard library only.

## hover-page

```
hover-page click TARGET                 click the best match ("watch later", "the menu")
hover-page focus TARGET                 focus the best-matching text field
hover-page type TARGET TEXT [--submit]  fill a field; --submit presses Enter / submits its form
hover-page type --split WORDS...        first 1-5 words name the field, the best split wins
hover-page scroll up|down|top|bottom [N|a bit|a lot]   N screens (0.8 of a viewport each)
hover-page back|forward|reload
hover-page list [TARGET]                candidates and scores, for debugging
  --dry      print what would be hit, change nothing      --timing  per-phase latency
  --embed    blend in EmbeddingGemma similarity           --notify  desktop note on a miss
  --tab TEXT act on the tab whose URL contains TEXT (default: the selected tab)
```

It prints one line saying what it did (`clicked link "Subscriptions" score 1.06`).
Exit 0 means it acted, 1 means nothing matched well enough, and 2 is an error
(hover not listening, the page changed under it).

### How it finds things

1. The selected tab comes from hover's chrome (the remote agent's
   `NavigableManager`), so split view acts on the focused pane. Without
   `--remote-allow-system-access` it falls back to the first visible tab.
2. One script in a BiDi sandbox (the page cannot see or tamper with it) collects
   visible links, buttons, inputs, `[role=button|link|tab|menuitem|searchbox|...]`,
   `[onclick]`, `summary`, contenteditable and `[aria-label]` elements, open shadow
   roots included. It skips anything more than 4 screens away from the view.
   Each element is named by aria-labelledby, aria-label, its `<label>`, visible text,
   value, title, placeholder, alt, and (with less weight) name/id attributes.
3. Python scores every candidate against the spoken target:
   - word overlap with per-word fuzzy matching ("subscription" matches "Subscriptions"),
     tolerant of ASR splitting or joining words ("fire ship", "e mail", "low fi");
   - a whole-string `difflib` ratio;
   - role words: *button, link, field/box/bar, tab, checkbox, menu/hamburger/sidebar,
     icon/logo*. They are not label text, but they favour elements of that kind.
     "search bar" prefers a search input, and a bare "menu" looks for the page's
     hamburger/guide/navigation toggle rather than any button whose label has
     "menu" in it;
   - small preferences for elements in the viewport, larger ones, and real controls
     over plain labelled elements.

   `focus` and `type` only consider editable fields. The best match needs a score
   of 0.55 or more.
4. It scrolls the element into view, outlines it in magenta for 600 ms so you can see
   what was hit, then clicks it (`el.click()`, fields are focused too) or fills it.
   Fields are filled through the native value setter plus `input`/`change` events,
   so React-style pages notice. contenteditable gets `insertText`.

### Matching accuracy

The fixtures in `tests/fixtures/` are a YouTube-like header and guide, a
docs page with sidebar navigation, and a sign-up form. Each has a labelled set of
spoken targets in `tests/cases.py`, including ASR-style noise and targets that
must not match. The scorer was tuned on the first set. The held-out set was
written afterwards and scored once.

| set | fuzzy (default) | `--embed` |
|---|---|---|
| tuning, 83 targets | 82 (98.8%) | 81 (97.6%) |
| held-out, 27 targets, blind run | 24 (88.9%) | 22 (81.5%) |
| held-out after two general fixes* | 26 (96.3%) | 24 (88.9%) |

\* "sidebar" became a menu word and submit buttons answer to "submit".

`--embed` re-ranks the top 40 fuzzy candidates with EmbeddingGemma
(`http://localhost:30201/v1/embeddings`, model `embeddinggemma-2`; override with
`HOVER_PAGE_EMBED_URL` / `HOVER_PAGE_EMBED_MODEL`), as 0.6 fuzzy + 0.4 cosine. It
recovered none of the fuzzy misses and added false positives: "upload video" hit
"Liked videos", "dark mode" hit "Run this code", "delete account" hit "Log in". It
also costs about 160 ms. Fuzzy stays the default. The remaining misses are a
phonetic mishearing ("in arms" for *Enums*) and a synonym ("surname" for *Last
name*).

On live pages (in a throwaway headless hover): YouTube's "menu" hits the Guide
button, "search bar" its search box, "subscriptions" the guide link. On
Wikipedia, "menu" hits *Main menu*, an opacity-0 checkbox laid over its label.

### Latency

Wall clock for the whole process, median of 20, on Centurion:

| command | time |
|---|---|
| Python startup alone | 43 ms |
| click / type / focus on a small page | 53-56 ms |
| scroll, back, forward, reload | ~50 ms |
| click on live YouTube | 65-73 ms |
| click on a large Wikipedia article (8k elements, 291 in reach) | ~100-115 ms |
| `--embed` | +160 ms (first call after idle: up to 2 s) |

`hover-page` speaks WebSocket itself instead of importing `websockets`, which
cost 80 ms per call to import.

## Phrases (hover-page.toml)

All phrases need hover to be under the pointer or focused (`app = "hover"`). They
must match the whole utterance after voice-cmd's normalization: lowercase, no
punctuation, filler like "please"/"okay"/"now" stripped, "one".."ten" as digits.

| say | does |
|---|---|
| *click / tap / press (on) X*, *go to X* | `click X` (X up to 6 words) |
| *focus (on) X* | `focus X` |
| *menu*, *open the menu*, *hamburger (menu)*, *open the main menu* | `click menu` |
| *open the X menu* | `click "X menu"` ("open the account menu") |
| *search (for) X*, *search youtube / this page for X* | `type --submit search X` |
| *type into X TEXT*, *type in X TEXT* | `type --split X TEXT` |
| *type TEXT into (the) X*, *type TEXT in the X field/box/bar* | `type X TEXT` |
| *scroll up/down (N / a bit / a lot) (pages)*, *page up/down* | `scroll` |
| *go to the top/bottom*, *scroll to bottom*, *top of the page* | `scroll top/bottom` |
| *(go) back*, *(go) forward*, *reload / refresh (the page)* | history, reload |

Things deliberately left out, or that lose to other commands:

- `~/.config/voice-commands` loads before voice-cmd's own `commands/`, and on equal
  score the first command wins. So these phrases exclude what hover.toml and
  hyprland.toml say: *open (the sidebar)*, *show the sidebar*, *go to workspace 3*,
  *focus left*.
- *press enter/escape/tab/...* is not a click. *back 3* is ears' delete-words command
  and ears handles it before voice-cmd.
- A bare *in* does not split text from field ("type checking in rust is great" stays
  dictation). Targets are capped at a few words, so long sentences that start with
  *click*, *search for* or *go to* pass through.
- *click like* loses its last word to voice-cmd's filler stripping (it becomes *click*
  and doesn't match). Say *click the like button*.

### Adding phrases

Add a `[[command]]` table to `hover-page.toml` (or another file in
`~/.config/voice-commands/`):

```toml
[[command]]
name = "page.subscribe"
app = "hover"
phrases = ["subscribe(?: to (?:this|the) channel)?"]
run = ["hover-page", "--notify", "click", "subscribe button"]
```

Use `(?:...)` for groups you don't pass on. Capturing groups are numbered `{1}`,
`{2}`, ... in `run`. Then add the phrase and a near-miss to `tests/test_phrases.py`
and check that nothing existing changed hands:

```bash
voice-cmd list                                   # everything loaded
VOICE_CMD_APP=hover voice-cmd match "subscribe"  # what an utterance resolves to
```

## Tests

```bash
python3 -m unittest discover -s tests            # offline: matching + phrase routing
python3 tests/test_matching.py --report --embed  # accuracy per set, fuzzy vs embed
```

`test_matching` scores against candidates captured from the fixture pages
(`fixtures/*.cands.json`). `test_phrases` routes utterances through the installed
voice-cmd with this plugin in front of its stock commands. The live tests really
click, type and scroll, so run them against a throwaway headless hover, never your
own session:

```bash
hover --headless --no-remote --profile "$(mktemp -d)" \
      --remote-debugging-port 9334 --remote-allow-system-access about:blank &
HOVER_PAGE_LIVE_PORT=9334 python3 -m unittest discover -s tests
python3 tests/capture.py 9334    # after changing the collector or a fixture
```

## Caveats

- voice-cmd runs commands detached and reports "handled" as soon as a phrase
  matches. A *click X* that finds nothing still swallows the utterance; `--notify`
  at least says so. The same goes for dictation that happens to be a whole phrase,
  e.g. *search for flights* spoken into a text box on a page that has a search field.
- Same-origin and cross-origin iframes are not searched.
- Elements more than 4 screens away are ignored. Scroll closer first.
