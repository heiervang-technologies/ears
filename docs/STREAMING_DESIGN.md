# Streaming Transcription + VAD Mode Design

## Overview

This document outlines the design for adding real-time streaming transcription with Voice Activity Detection (VAD) to ears.

## Requirements

Based on user input:
1. **Streaming transcription**: Progressive text appearing as the user speaks
2. **VAD mode**: Auto-detect speech segments, toggle on/off via keyboard shortcut
3. **LocalAgreement policy**: Only commit/type text when it's stable across multiple iterations
4. **Dual display**: Show in TUI AND type progressively into active window
5. **Support both backends**: whisper.cpp and faster-whisper
6. **Correction toggle**: Eventually add ability to disable auto-correction in TUI

## Architecture

### State Machine Extension

Current states: `Idle`, `Recording`, `Transcribing`

New state: `VadActive`

```
┌──────┐  toggle  ┌───────────┐
│ Idle │ ────────▶│ Recording │
└──────┘          └───────────┘
   │                    │
   │ toggle             │ stop
   │ (VAD mode)         ▼
   │              ┌──────────────┐
   └─────────────▶│ Transcribing │
                  └──────────────┘
   ┌──────────┐         │
   │ VadActive│◀────────┘
   └──────────┘
       │ toggle
       │ (disable VAD)
       ▼
   ┌──────┐
   │ Idle │
   └──────┘
```

### Component Architecture

```
┌─────────────────────────────────────────────────────────┐
│                     ears Main Process                   │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌────────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │   State    │   │  Streaming   │   │     VAD      │ │
│  │  Manager   │◀──│   Engine     │◀──│   Detector   │ │
│  └────────────┘   └──────────────┘   └──────────────┘ │
│         │                 │                    │       │
│         ▼                 ▼                    ▼       │
│  ┌────────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │   Config   │   │ LocalAgree   │   │   PipeWire   │ │
│  │            │   │   Policy     │   │   Capture    │ │
│  └────────────┘   └──────────────┘   └──────────────┘ │
│                           │                            │
│                           ▼                            │
│                   ┌──────────────┐                     │
│                   │   Whisper    │                     │
│                   │   Backend    │                     │
│                   └──────────────┘                     │
│                    /            \                      │
│                   /              \                     │
│         ┌────────────┐     ┌──────────────┐           │
│         │ whisper.cpp│     │faster-whisper│           │
│         │   stream   │     │  + WhisperX  │           │
│         └────────────┘     └──────────────┘           │
│                                                        │
│  ┌─────────────────────────────────────────────────┐  │
│  │                    Output                        │  │
│  │  ┌──────────────┐           ┌─────────────────┐ │  │
│  │  │     TUI      │           │    ydotool      │ │  │
│  │  │ Live Panel   │           │ Progressive Type│ │  │
│  │  └──────────────┘           └─────────────────┘ │  │
│  └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

## Implementation Strategy

### Phase 1: Core Infrastructure (Week 1)

1. **Extend State enum**
   - Add `VadActive` state
   - Update StateManager to handle new transitions
   - Add VAD mode configuration

2. **Create streaming module** (`src/streaming.rs`)
   - `StreamingEngine`: Main coordinator
   - `AudioBuffer`: Circular buffer for audio chunks
   - `TranscriptBuffer`: Tracks stable vs unstable text
   - `LocalAgreementPolicy`: Implements consensus logic

3. **VAD Integration**
   - Option A: Use whisper.cpp built-in VAD (simpler)
   - Option B: Integrate Silero-VAD (more flexible)
   - Detect speech start/stop events

### Phase 2: Whisper Backend Integration (Week 1-2)

4. **whisper.cpp streaming**
   - Modify WhisperClient to support streaming endpoint
   - Chunked audio upload (e.g., 500ms chunks)
   - Parse incremental responses

5. **faster-whisper streaming**
   - Option A: Integrate WhisperLive client library
   - Option B: Build custom client using faster-whisper API
   - Support for both backends via trait abstraction

### Phase 3: UI & UX (Week 2)

6. **TUI Live Transcription Panel**
   - New `Panel::LiveTranscription`
   - Display:
     - Committed text (white/green)
     - Uncommitted text (gray/dimmed)
     - VAD status indicator
     - Correction toggle checkbox

7. **Progressive Typing**
   - Track last committed position
   - Only type stable prefix via ydotool
   - Handle text corrections (backspace + retype)

8. **Correction Toggle**
   - TUI checkbox: "Auto-type corrections"
   - If disabled: Only append, never backspace
   - If enabled: Fix mistakes in real-time

## LocalAgreement Policy Details

### Algorithm

```rust
struct LocalAgreementPolicy {
    n: usize,  // Agreement threshold (default: 2)
    history: VecDeque<String>,  // Last n transcripts
    committed: String,  // Stable prefix
}

impl LocalAgreementPolicy {
    fn process(&mut self, new_transcript: String) -> (String, String) {
        self.history.push_back(new_transcript.clone());
        if self.history.len() > self.n {
            self.history.pop_front();
        }

        // Find longest common prefix across all history
        let stable_prefix = self.find_common_prefix();

        // Calculate what's new vs what's uncommitted
        let newly_committed = &stable_prefix[self.committed.len()..];
        let uncommitted = &new_transcript[stable_prefix.len()..];

        self.committed = stable_prefix;

        (newly_committed.to_string(), uncommitted.to_string())
    }

    fn find_common_prefix(&self) -> String {
        if self.history.len() < self.n {
            return String::new();
        }

        // Find longest common prefix of last n items
        let mut prefix = self.history[0].clone();
        for transcript in &self.history[1..] {
            prefix = common_prefix(&prefix, transcript);
        }
        prefix
    }
}
```

### Example Flow

```
Iteration 1: "Hello"
  - History: ["Hello"]
  - Committed: "" (need 2 agreements)
  - Display: "Hello" (gray/uncommitted)

Iteration 2: "Hello wo"
  - History: ["Hello", "Hello wo"]
  - Committed: "Hello"
  - Newly committed: "Hello"
  - TYPE: "Hello"
  - Display: "Hello" (white) + " wo" (gray)

Iteration 3: "Hello world"
  - History: ["Hello wo", "Hello world"]
  - Committed: "Hello wo"
  - Newly committed: " wo"
  - TYPE: " wo"
  - Display: "Hello wo" (white) + "rld" (gray)

Iteration 4: "Hello world"
  - History: ["Hello world", "Hello world"]
  - Committed: "Hello world"
  - Newly committed: "rld"
  - TYPE: "rld"
  - Display: "Hello world" (all white)
```

## Audio Processing Pipeline

```
Microphone
    │
    ▼
PipeWire Capture (continuous, 16kHz mono)
    │
    ▼
Circular Buffer (10 seconds capacity)
    │
    ▼
VAD Detector (50ms frames)
    │
    ├─ Speech Detected ──▶ Extract chunk (500ms-3s)
    │                          │
    │                          ▼
    │                      Whisper API
    │                          │
    │                          ▼
    │                      Transcript
    │                          │
    │                          ▼
    │                      LocalAgreement
    │                          │
    │                          ├─ Stable ──▶ Type via ydotool
    │                          │
    │                          └─ Unstable ─▶ Show in TUI (gray)
    │
    └─ Silence ──▶ Wait / No action
```

## Configuration

New config options in `~/.config/ears/config.toml`:

```toml
[vad]
enabled = true
threshold = 0.5  # Silence threshold (0.0-1.0)
min_speech_duration_ms = 300  # Minimum speech segment
max_silence_duration_ms = 700  # Max silence before segment end

[streaming]
chunk_size_ms = 500  # Audio chunk size for transcription
buffer_size_seconds = 10  # Max audio buffer
agreement_threshold = 2  # LocalAgreement-n parameter
progressive_typing = true  # Enable real-time typing
auto_correction = true  # Allow backspace corrections
```

## TUI Updates

New panel: **Live Transcription**

```
┌─────────────────────────────────────────────────┐
│ Status │ Configuration │ Logs │ Live │          │
└─────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────┐
│ VAD Mode: ● Active                              │
│                                                 │
│ Transcription:                                  │
│                                                 │
│ Hello world, this is a test                     │
│ of the streaming transcription sys              │
│  └─ (gray = uncommitted)                        │
│                                                 │
│ Settings:                                       │
│ [x] Progressive Typing                          │
│ [x] Auto-correction                             │
│ [ ] Show timestamps                             │
│                                                 │
│ Stats:                                          │
│ Latency: 280ms                                  │
│ Segments processed: 42                          │
│ Accuracy: 98.2%                                 │
└─────────────────────────────────────────────────┘
│ [Space] Toggle VAD │ [t] Toggle typing │ [q] Quit │
└─────────────────────────────────────────────────┘
```

## Keyboard Shortcuts

| Key | Mode | Action |
|-----|------|--------|
| Shortcut (1st press) | Idle | Start recording (current behavior) OR toggle VAD mode (new) |
| Shortcut (2nd press) | Recording | Stop & transcribe (current) |
| Shortcut (toggle) | VAD Active | Disable VAD mode |
| `Space` (in TUI) | Any | Toggle VAD mode |
| `t` (in TUI) | VAD Active | Toggle progressive typing |
| `c` (in TUI) | VAD Active | Toggle auto-correction |

## Progressive Typing Behavior

### With Auto-Correction Enabled (default)

```
User says: "Hello world"
Whisper hears: "Hello word" → types "Hello word"
Next iteration: "Hello world" → backspace 2, type "rld"
Final: "Hello world"
```

### With Auto-Correction Disabled

```
User says: "Hello world"
Whisper hears: "Hello word" → types "Hello word"
Next iteration: "Hello world" → do nothing (don't correct)
Final: "Hello word" (keeps mistake, faster)
```

## Backend Abstraction

```rust
trait StreamingBackend {
    async fn stream_transcribe(
        &self,
        audio_chunk: &[f32],
    ) -> Result<TranscriptChunk>;

    fn supports_vad(&self) -> bool;
}

struct WhisperCppStreaming {
    client: WhisperClient,
    vad_enabled: bool,
}

struct FasterWhisperStreaming {
    whisperlive_client: WhisperLiveClient,
}

impl StreamingBackend for WhisperCppStreaming { ... }
impl StreamingBackend for FasterWhisperStreaming { ... }
```

## Testing Strategy

1. **Unit tests**
   - LocalAgreement policy correctness
   - Audio buffer management
   - VAD detection accuracy

2. **Integration tests**
   - End-to-end streaming with mock audio
   - TUI live panel rendering
   - Progressive typing simulation

3. **Manual testing**
   - Real-time latency measurement
   - Accuracy comparison (streaming vs batch)
   - User experience testing

## Performance Targets

- **Latency**: < 500ms from speech end to typing start
- **Accuracy**: Within 2% of batch transcription
- **CPU**: < 10% overhead (VAD + buffer management)
- **Memory**: < 100MB additional for 10s audio buffer

## Open Questions

1. Should VAD mode be a separate toggle, or replace the current recording mode?
   - **Proposal**: Separate toggle (press and hold vs single press)

2. How to handle very long speech segments (> 30 seconds)?
   - **Proposal**: Auto-segment every 10 seconds, use overlapping windows

3. What to do if whisper.cpp doesn't support streaming natively?
   - **Proposal**: Implement our own chunking + LocalAgreement wrapper

4. Should we support multiple VAD backends?
   - **Proposal**: Start with one (whisper.cpp built-in or Silero), add more later

## References

- [whisper_streaming](https://github.com/ufal/whisper_streaming) - LocalAgreement implementation
- [WhisperLive](https://github.com/collabora/WhisperLive) - Near-real-time transcription
- [Silero VAD](https://github.com/snakers4/silero-vad) - Voice activity detection
- [whisper.cpp stream example](https://github.com/ggml-org/whisper.cpp/tree/master/examples/stream)
