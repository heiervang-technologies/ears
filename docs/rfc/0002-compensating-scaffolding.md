# RFC 0002: Compensating Scaffolding (cs)

Status: accepted for implementation, 2026-11-13. Scope of the first version:
command-like matching only (wake, sleep, standby, the voice commands, run
commands, command-mode actions, decide options). Dictated text is never
rewritten.

## Problem

The ASR mishears short, out-of-vocabulary command phrases in repeatable
ways, and the configuration compensates by hand. On centurion the `wake`
list has 15 entries, most of them mishearings ("hay clanker", "hey glinker",
"adsense", "h e y clanker"); `sleep` has 13 ("years off", "fuck off kleiner",
"yourself"). About 40 `config.toml.bak-*` files exist, most made to add one
variant. Nothing records why a variant exists, which ASR produced it, how
often, or whether it collides with ordinary speech ("yourself" as a sleep
phrase can fire mid-dictation).

**Compensating Scaffolding** is static, auto-annotated data that maps what a
given ASR model hears to what was meant, generated and checked by tools
rather than typed by hand.

## Data format

TOML, one file per (model, domain). Entries are only ever *additional
phrasings* of an existing canonical phrase; they never introduce a new
action.

```toml
[meta]
model = "qwen3-asr-1.7b"       # ASR model id (first id of the server's /v1/models), or "any"
domain = "commands"            # first version: "commands" only
generated = "2026-11-13"
generator = "ears cs build"    # or "hand", "ears cs migrate", "ears cs mine"

[[entry]]
meant = "ears on"              # canonical phrase, exactly as configured
target = "wake"                # see Targets
heard = "adsense"              # normalized with commands::normalize
mode = "hallucination"         # see Failure modes
source = "hand"                # hand | probe | mined
count = 0                      # times observed (probe hits or mined events)
trials = 0                     # probe trials for `meant` (0 for hand/mined)
risk = 0.0                     # 0..1 from cs::risk::assess
risk_reason = ""
status = "active"              # active | flagged | rejected
```

**Targets** name the list an entry extends: `wake`, `sleep`, `standby`,
`enter`, `new_line`, `undo`, `back`, `backtrack`, `sign_off`, `literal`,
`steer`, any other `[commands]` list key, `run:<name>` for run commands,
`action:<name>` for command-mode actions (actions.toml), and
`decide:<option>` for `ears decide` options.

**Failure modes**: `homophone` (years/ears), `split` (back track), `spelled`
(v a d), `name_drift` (clanker -> klinker), `truncation`, `hallucination`
(adsense), `other`.

**Status**: only `active` entries are applied. `flagged` entries are shown by
`ears cs check` for review. `rejected` entries are kept so tools do not
propose them again.

## Where files live and how they combine

1. Built-in defaults, embedded in the binary (the binary is copied to three
   paths on some desks; it must not depend on a repo checkout). Source of
   truth in the repo: `cs/<model>/<domain>.toml` and `cs/any/<domain>.toml`.
2. User files: `~/.config/ears/cs.d/*.toml`.

The model is resolved from the configured ASR server's `/v1/models` once per
VAD start (cached; failure means "unknown"). Files whose `meta.model` matches,
plus all `any` files, apply. A user entry with the same (target, heard)
overrides a built-in one, including to set `status = "rejected"`.

## How it is applied: one place, at load time

No new pipeline stage. When the phrase lists for matching are built (the
`[commands]` lists in `commands.rs`, wake phrases in `wake.rs`, run commands,
command-mode action phrasings, decide options), each list is extended with
the `heard` of every active entry for its target. Matching itself is
unchanged. Existing hand-written variants in `config.toml` keep working.

## Module layout and interfaces

All under `src/cs/`. Each part declares its own `pub mod` line in
`src/cs/mod.rs`; trivial conflicts there are resolved at merge.

- `src/cs/mod.rs` (core): types, (de)serialization, loading and merging,
  `Scaffold::variants`, embedding defaults, wiring into the list builders,
  `ears cs check` and `ears cs migrate`.

  ```rust
  pub enum FailureMode { Homophone, Split, Spelled, NameDrift, Truncation, Hallucination, Other }
  pub enum Source { Hand, Probe, Mined }
  pub enum Status { Active, Flagged, Rejected }
  pub struct Entry { pub meant: String, pub target: String, pub heard: String,
      pub mode: FailureMode, pub source: Source, pub count: u32, pub trials: u32,
      pub risk: f32, pub risk_reason: String, pub status: Status }
  pub struct Meta { pub model: String, pub domain: String, pub generated: String, pub generator: String }
  pub struct CsFile { pub meta: Meta, pub entry: Vec<Entry> }
  pub struct Scaffold { /* merged, for one model */ }
  impl Scaffold {
      pub fn load(model: Option<&str>) -> Scaffold;
      pub fn variants(&self, target: &str) -> Vec<&str>;   // active only
      pub fn entries(&self) -> impl Iterator<Item = &Entry>;
  }
  ```

- `src/cs/risk.rs` (risk scoring, pure, no I/O):

  ```rust
  pub struct Phrase<'a> { pub text: &'a str, pub target: &'a str }
  pub enum Verdict { Ok, Flag, Reject }
  pub struct Risk { pub score: f32, pub verdict: Verdict, pub reason: String }
  pub struct Lexicon { /* common English words/bigrams with frequencies */ }
  impl Lexicon { pub fn builtin() -> Lexicon; }
  pub fn phonetic_key(text: &str) -> String;
  pub fn phonetic_distance(a: &str, b: &str) -> f32;          // 0 = same sound, 1 = unrelated
  pub fn assess(heard: &str, target: &str, others: &[Phrase], lexicon: &Lexicon) -> Risk;
  ```

  `assess` must reject an exact collision with another target's phrase, flag
  near-collisions by sound, and flag variants that are common in ordinary
  speech (a single frequent word like "yourself", or a frequent bigram), and
  shorter, less distinctive variants more. Deterministic, documented,
  calibrated with tests on the real lists in this RFC.

- `src/cs/probe.rs` + `ears cs build` (generator): speaks each canonical
  phrase through TTS with several voices, speeds and noise levels into the
  configured ASR, collects what comes back, and writes an annotated
  `cs/<model>/commands.toml` (source `probe`, counts and trials filled,
  failure mode classified, risk from `cs::risk`).

- `src/cs/mine.rs` + `ears cs mine` (opt-in): a local utterance journal
  (`[cs] journal = true`, default off, bounded retention, under
  `~/.local/state/ears/`), and a miner that proposes entries from "unmatched
  utterance followed within a few seconds by a matched one" and from chant
  repeats. Output is a review file with `status = "flagged"`; nothing mined is
  applied automatically.

- Prevention (separate from cs files): in command mode, pass the active
  phrases as ASR context (the stream protocol's `context`), measured against
  the same probe set with and without context.

## Acceptance for the first version

- Existing configs behave identically with no cs files present.
- `ears cs check` lists every entry with its risk, and flags "yourself" (sleep).
- `ears cs migrate` turns the hand variants in a user's `config.toml` into a
  reviewable cs file (heard -> meant by phonetic nearest canonical, source
  `hand`), without changing `config.toml`.
- Generated defaults for the current ASR model ship embedded, every entry with
  a risk below the reject threshold.
