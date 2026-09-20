# VAD reliability and daily use

Status: implementation underway on isolated review branches, 2026-09-20.
The reported intermittent missed speech has not been reproduced or attributed
to a confirmed cause. The running installation has not been replaced.

Current patches implement independent health supervision, versioned desktop
ownership/progress snapshots, bridge validation for Friend, capture lifecycle
repair, bounded desktop helpers, and rejected-candidate volume restoration.
Diagnostics preserve VAD segmentation in replay tests. The snapshot reports
latest levels/probability and cumulative rejection counts; it is not an audio
recording or a complete per-frame trace. Explicit audio incident capture,
restart controls, queue decoupling, and sensitivity changes remain future work.
Reviewed collaboratively by Ears Astra and Ears Fable through Director.

## Evidence from the current implementation

- `SileroVad::process_frame` uses a fixed threshold. Before speech starts, a
  below-threshold frame resets the consecutive-positive counter. Defaults of
  0.5 and 300 ms require ten 32 ms positive frames (320 ms).
- `StreamingEngine::process_audio` awaits transcription and typing before
  processing the next audio chunk. Capture continues into an unbounded queue.
- Capture read failures are logged, but `ContinuousCapture` retains an audio
  sender after its reader exits. The consumer can therefore wait indefinitely
  rather than observe channel closure. The UI and headless supervisor do not
  monitor pipeline completion during normal operation.
- Capture stderr is discarded. Existing debug logging records transitions,
  but not continuous evidence that audio and inference are advancing.
- The reader takes ownership of the capture child out of `self.process`.
  Consequently `is_running()` reports false after startup and `stop()`/Drop
  cannot terminate that child. Health reporting must not rely on this method
  until ownership and lifecycle tracking are repaired.
- Speech-like fixture tests currently allow zero detected speech. They cannot
  establish that a second or third utterance is still recognized.
- Probable speech triggers volume ducking. A rejected candidate does not emit
  a matching end event and can leave output ducked until a later completed
  utterance; this is separate from microphone detection.
- Typing and desktop detection use blocking subprocess calls without deadlines.
  A stuck child can stop pipeline progress. A timeout around `spawn_blocking`
  alone does not cancel the operation: recovery must kill and reap the owned
  child and explicitly handle partially delivered text.

## Friend integration and existing audio cues

Preserve the existing open/close, probable/confirmed speech, and speech-end
audio cues. Add health reporting without duplicating or changing their meaning.
In particular, do not reuse a completed-speech event for a rejected candidate
without considering its audible and external-consumer effects. Give candidate
cancellation explicit internal semantics so ducking can restore correctly.

The checked-in Friend integration follows this path:

`Ears state file -> haios-ears-bridge -> haios-state -> Friend`

Friend invokes `ears vad` to toggle listening. The bridge currently gates a
plain state-file reading on the existence of any executable named `ears`.
This does not establish microphone or detector health, and another instance
such as `ws-listen` can satisfy that process check. Its publication TTL measures
bridge freshness, not capture freshness. The VAD state file remains
`vad_active` during segment transcription, so it does not convey that stage.

Add a versioned health snapshot or compatible status query owned by the actual
desktop capture session. Include process/session identity, capture progress,
detector progress, processing stage, and failure reason. The bridge should
consume those facts and publish existing bus roles with clear detail text.
On loss of telemetry, withdraw the listening claim or report unknown; a fresh
bridge poll must never renew stale pipeline evidence. Disabled or uninstalled
voice input remains a normal state, not a desktop fault.

As an earlier, independent fix, publish desktop-owner metadata in a separate,
atomically replaced sidecar: PID, process start identity (to reject reused PIDs),
session identifier, and session kind. Keep the existing plain state file
compatible. The bridge must correlate state with its actual owner; test owner
changes and partially completed updates. Process identity closes the unrelated
`ws-listen` loophole but does not replace the later progress heartbeat.

Keep existing IPC event names and payloads compatible. Verify bridge parsing
and Friend rendering before adding events or roles. Use the current role
manifest where possible; new roles require coordinated bridge/manifest/Friend
updates. Friend restart/reconnect must obtain a current snapshot rather than
depend on having observed earlier transition events.

Integration gates: desktop capture dead while `ws-listen` survives; frozen
pipeline with a live process; bridge restart; Friend restart; old/new protocol
versions; ordinary voice disable; no duplicate cue or text delivery.

## Small, separately reviewable changes

### 1. Make failures distinguishable

Add opt-in diagnostic summaries, with a session identifier and monotonic times:

- Capture: samples received, age of last chunk, RMS/peak level, read errors.
- Detection: frames processed, probability range, threshold crossings,
  candidate resets, confirmed speech state and current segment duration.
- Processing: segment identifier, current stage, elapsed time and queue age.

Emit periodic summaries at most once per second, plus stage transitions and
errors. Keep diagnostic state bounded and avoid per-frame disk writes. The
capture heartbeat must advance independently of transcription; its absence
must be distinguishable from a microphone delivering silence. Report stalled
work from a supervisor rather than from the potentially stalled task itself.
Publish a consistent stage/timestamp pair before each potentially blocking
operation, so the supervisor can identify where progress stopped. Use atomics
with a consistent snapshot protocol or a short-lived lock never held across
external work; the supervisor must remain independently runnable.
Do not put audio or transcript contents into new diagnostic records by default.
Explicit incident capture can retain a bounded audio window for local replay.

Acceptance: diagnostics on/off produce identical segments on the same input;
logs identify capture starvation, low-confidence input, and slow downstream
work separately. Record overhead before enabling periodic reporting by default.

### 2. Repair lifecycle handling and make listening status truthful

Implement these as independent, focused fixes before queue restructuring:

- Give capture an explicit child owner and cancellation path that kills and
  reaps it; track reader liveness separately and propagate its exit reason.
- Bound typing and desktop-probe subprocess waits with owned-child kill/reap.
  Move blocking work off async workers without mistaking task timeout for child
  cancellation. Report partially delivered text; never retry injection blindly.
- Restore ducked volume on rejected candidates, stop, and failure. Serialize
  duck/restore operations or invalidate pending operations so a late duck
  cannot run after restoration. Preserve existing speech-completion cues.
- Cache desktop capability detection per session, with bounded initial probes
  and explicit refresh on relevant settings changes or backend failure.
- Add the correlated desktop-owner metadata described above to disambiguate
  desktop VAD from other Ears processes.

Propagate capture termination and pipeline task failure to the owner. Show
plain states such as Listening, Hearing speech, Transcribing, and Microphone
disconnected. Account for capture and transcription separately if concurrent.
Expose equivalent health information for headless use. Include microphone name
and a level meter so a wrong or silent input is visible.

Give the user an explicit restart-listening action. Consider bounded automatic
capture recovery only after termination and shutdown tests pass. Never restart
just because speech probability is low or replay text already delivered.

Acceptance: capture EOF, capture error, and pipeline failure leave no false
healthy-listening indication; user-requested stop completes promptly. A hung
fake typing child is terminated and reaped without repeated text delivery;
rejected speech restores volume, including when a duck command completes late.

### 3. Keep detection responsive during downstream work

Separate detection from transcription/output using an ordered, bounded segment
queue. Preserve utterance order and existing typing semantics. Define queue
overflow visibly; never silently drop speech or accumulate unlimited audio.
Keep shutdown independent of slow backend work, retaining the subprocess
deadlines introduced in the lifecycle fixes.
Do not retry text injection automatically: a partially typed result could be
duplicated. Treat this as a distinct architecture change after diagnostics.

### 4. Change sensitivity only with replay evidence

Compare baseline and candidate behavior using real, consented recordings of
quiet speech, short commands, pauses, background sounds, and repeated
utterances. Consider hysteresis or limited tolerance for low-confidence gaps
only if failures actually occur in detection. Keep the existing defaults until
the comparison establishes the tradeoff in missed speech and false triggers.

## Regression gates

- Deterministic probability-sequence tests: candidate rejection, confirmation,
  silence termination, and successful second/third utterances.
- Real speech replay: assert segment count and approximate boundaries, not
  merely absence of a panic. Feed audio in realistic chunks and vary framing.
- Fault injection: reader EOF/error, silent input, slow/failed transcription,
  typing failure, and shutdown during each stage.
- Repeated lifecycle tests: start/stop, device loss/recovery, no leaked capture
  processes, and no repeated text delivery.
- Ducking tests: rejected candidates, delayed volume commands, and shutdown
  restore volume correctly using a fake volume backend.
- Compare missed utterances, false triggers, detection-to-output latency,
  maximum backlog, and stop latency on the same corpus before/after each patch.

Ship observability, lifecycle fixes, queue changes, and detection changes
separately. Preserve a known working binary and configuration for rollback.
