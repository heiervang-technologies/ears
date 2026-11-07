# Mining compensating scaffolding (`ears cs mine`)

Part of RFC 0002 (compensating scaffolding). Off by default. The keys sit in
the same `[cs]` section as `context`. Code:
`src/cs/mine.rs`.

## The journal

```toml
[cs]
journal = true          # default false
journal_max_kb = 1024   # rotate past this; one old file is kept
journal_max_days = 30   # lines older than this are dropped
mine_window_s = 6       # ears cs mine: retry window, seconds
mine_min_count = 2      # ears cs mine: least evidence per proposal
mine_max_distance = 0.35  # sound-alike limit, pairing and standby
```

VAD then appends one JSON line per finished utterance to
`$XDG_STATE_HOME/ears/cs-journal.jsonl` (`~/.local/state/ears/` when
`XDG_STATE_HOME` is unset). The file is mode 0600, never larger than about
twice `journal_max_kb` (current plus `cs-journal.jsonl.1`), and lines older
than `journal_max_days` are removed at VAD start and hourly.

A line holds the time, the mode, the words (normalized), what they matched
(`target` and `phrase`, or nothing), how they matched when not by their
words (`via`), and in command mode the routing decision also written to
`command-last.json` (`routing`: the words, the audio and text votes with
their margins, what fired).

Writing never delays the audio path: lines go through a bounded queue to a
writer thread and are dropped when it is full. Drops are counted; the next
written line carries the count in `dropped`.

### What is kept, and what never is

Dictated text is personal and is never journaled. Kept are:

| mode | kept |
|------|------|
| standby | matched utterances; unmatched ones only if they sound like a standby phrase (see below) |
| command | every utterance with its routing, except `ears teach` recordings |
| decide (`ears decide`) | option picks, cancels and unclear answers; **not** an answer of your own |
| dictation, vision | only an utterance that *is* a voice command (wake, sleep, "command", a colour, enter, undo, a run command...) |

Of a run command that passes the rest of the utterance on (`prefix`), and
of a command closing a dictated sentence ("... over"), only the command's
own words are kept. `literal ...` is dictation and is not kept.

Standby hears the room, so most of what it hears is overheard or private
conversation. An unmatched standby utterance is kept only when it sounds
like a try at a phrase standby listens for: its phonetic distance
(`cs::risk::phonetic_distance`) to the nearest wake, sleep, standby or run
phrase, or "command", is at most `mine_max_distance` (default 0.35).
"hay clanker" next to a "hey clanker" wake phrase is kept; "what time is
it" or "call mom" is dropped. Short phrases attract short speech ("come on" sounds like "command"), so
a false near miss can still slip through;
turn the journal off (and delete the file) if even that matters.

## The miner

```
ears cs mine [--since 7d] [--out FILE] [--journal FILE] [--model ID]
```

Evidence that `heard` was a mishearing of a phrase:

- **retry**: an unmatched utterance followed within `mine_window_s` seconds
  (default 6) by a matched one in the same mode, nothing matched between;
- **routed**: command mode or a decision fired by sound or meaning while the
  words were `heard`;
- **chant**: another reading of a chanted breath that fired (chant mode,
  ears#264; used once chant writes `alts`).

Each pair must also sound alike: phonetic distance at most
`mine_max_distance` (default 0.35), so "what time is it" followed by "ears
off" does not pair. Evidence is summed per (target, heard); at least
`mine_min_count` (default 2) makes a proposal, scored with
`cs::risk::assess` and classified by failure mode.

The review file (default `~/.config/ears/cs.d/mined.toml`) has
`source = "mined"` and `status = "flagged"` on every new entry: nothing
mined applies until you set `status = "active"`. Set `"rejected"` to stop
an entry being proposed again. Mining again keeps every entry you reviewed
as it is and refreshes the counts of those still flagged. A (target, heard)
that any other cs file already has (built in or in `cs.d`, any status, any
model: from `ears cs migrate`, `ears cs build` or by hand) is never
proposed again.
