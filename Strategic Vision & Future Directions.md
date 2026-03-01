# Strategic Vision & Future Directions

This document consolidates strategic architectural shifts and creative expansions for the `ears` project. These proposals address long-term performance, stability, and feature multiplier opportunities that go beyond immediate bug fixes. Human review and consensus are required before implementation.

## P5 & P6: Future Directions & Novel Expansions

### 1. Wayland Native Protocols (`zwp_virtual_keyboard_v1`)
**Problem:** Currently, `ears` relies heavily on invoking external CLI tools (`wtype`, `ydotool`) to simulate keyboard input. `wtype` is bound to specific Wayland compositors (like Hyprland) and `ydotool` requires a background daemon with root privileges (`ydotoold`). This makes the application fragile and dependent on system configuration.
**Proposal:** Leverage the `wayland-client` crate (or `smithay-client-toolkit`) to directly implement the `zwp_virtual_keyboard_v1` protocol.
**Impact:**
- Eliminates external binary dependencies.
- Achieves zero-latency text injection.
- Unifies Wayland support natively inside the Rust daemon.

### 2. Native Audio Processing (CPAL & Rodio)
**Problem:** Audio capture relies on spawning the `pw-record` subprocess, passing data via standard out or temporary files, and validating the `WAV` headers. Managing subprocess lifetimes introduces race conditions and zombie process risks.
**Proposal:** Adopt the `cpal` crate for native cross-platform audio capture, specifically binding to the PipeWire ALSA/Pulse layer or using PipeWire native Rust bindings.
**Impact:**
- Drastically reduces process management overhead.
- Enables in-memory audio buffering (avoiding disk I/O completely during VAD).
- Improves security by keeping audio streams entirely in user-space memory.

### 3. In-Flight Grammar Correction (Local LLM Integration)
**Problem:** Local ASR engines like `whisper.cpp` or `Qwen3-ASR` occasionally hallucinate filler words (e.g., "Um", "Thank you for watching") or fail to properly punctuate sentences based on the speaker's intent.
**Proposal:** Introduce a pre/post-processing pipeline step that pipes the raw transcribed text through a fast, locally hosted LLM (e.g., Llama 3 8B via Ollama). The LLM can be prompted to act as an advanced text filter.
**Impact:**
- Enables intelligent grammar correction.
- Allows for dictation commands (e.g., user says "delete last sentence", LLM executes the semantic action).
- Substantially improves the perceived intelligence and polish of the transcribed text.

### 4. Zero-Copy VAD Pipeline
**Problem:** The current Voice Activity Detection (VAD) pipeline handles continuous audio capture by framing, buffering, and moving `f32` samples across multiple channels.
**Proposal:** Refactor the internal VAD engine to use zero-copy ring buffers and `Arc<[f32]>` slices to eliminate allocation overhead during continuous capture mode.
**Impact:**
- Lowers CPU utilization and memory fragmentation on lower-end hardware, making `ears` highly efficient as a constant background daemon.