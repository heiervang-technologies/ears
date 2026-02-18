# Ears TUI Analysis & UX Improvement Proposals

## 1. Current State Overview

### Architecture

The TUI is built with **ratatui + crossterm** and uses a synchronous event polling loop with a 250ms tick rate. It supports keyboard (vim-style), mouse click, and vim command mode (`:q`, `:wq`, etc.) input.

### Layout Structure

The screen is divided into 4 vertical zones:

```
┌─────────────────────────────────────────────────┐
│ Header (3 lines) - App title + status indicator  │  Fixed
├─────────────────────────────────────────────────┤
│ Tabs (3 lines) - ▸ Status | Config | Logs | Live│  Fixed
├─────────────────────────────────────────────────┤
│                                                  │
│ Content (min 10 lines) - Active panel content    │  Flexible
│                                                  │
├─────────────────────────────────────────────────┤
│ Footer (3 lines) - Context-sensitive keybindings │  Fixed
└─────────────────────────────────────────────────┘
```

Total fixed chrome: **9 lines** of the terminal are always consumed by header + tabs + footer borders/padding, leaving only the remaining lines for actual content.

### Panels (Tabs)

| # | Panel | What it shows |
|---|-------|---------------|
| 1 | **Status** | Current state (Recording/Idle), Model, Server, Device, Language |
| 2 | **Configuration** | Server URL (editable), Model, Device (picker), Language, Text Filters (lowercase, punctuation) |
| 3 | **Logs** | Scrollable list of log messages |
| 4 | **Live** (Live Transcription) | VAD status, live transcription text, Settings (progressive typing, auto-correction), Stats (latency, segments) |

### Keybindings

**Global:**
| Key | Action |
|-----|--------|
| `q` / `Esc` / `Ctrl+C` | Quit |
| `h` / `Left` / `Shift+Tab` | Previous tab |
| `l` / `Right` / `Tab` | Next tab |
| `j` / `Down` | Scroll down (Logs panel only) |
| `k` / `Up` | Scroll up (Logs panel only) |
| `Space` | Toggle recording (Status) or Toggle VAD (Live) |
| `v` | Toggle VAD mode |
| `c` | Jump to Configuration panel |
| `Shift+L` | Cycle language |
| `:` | Enter command mode |

**Configuration panel only:**
| Key | Action |
|-----|--------|
| `e` | Edit server URL |
| `d` | Open device picker |
| `f` | Toggle lowercase filter |
| `p` | Toggle punctuation filter |

**Live panel only:**
| Key | Action |
|-----|--------|
| `t` | Toggle progressive typing |
| `a` | Toggle auto-correction |

**Device picker:**
| Key | Action |
|-----|--------|
| `j`/`k` / `Up`/`Down` | Navigate |
| `Enter` | Select |
| `Esc` | Cancel |

---

## 2. Issues Found

### Issue A: Massive Information Redundancy Between Status and Configuration Panels

**The Status and Configuration panels show nearly identical information:**

| Field | Status Panel | Configuration Panel |
|-------|-------------|-------------------|
| Current State | Yes ("Recording"/"Idle") | No |
| Model | Yes | Yes |
| Server URL | Yes | Yes (editable) |
| Device | Yes | Yes (with picker) |
| Language | Yes | Yes (with cycle hint) |
| Text Filters | No | Yes |

**4 out of 5** fields on the Status panel are duplicated verbatim on the Configuration panel. The only unique thing Status shows is "Current State: Recording/Idle" — but this is *already shown in the Header* as the status indicator (●/◉/○ with "VAD Active"/"Recording (Xs)"/"Idle").

**Verdict:** The Status panel is almost entirely redundant. It shows read-only versions of the same data that Configuration shows in editable form.

### Issue B: The Status Panel's "Current State" is Triple-Redundant

The current recording/VAD state is shown in **three places simultaneously**:
1. **Header** — "Status: ● Recording (5s)" / "Status: ◉ VAD Active" / "Status: ○ Idle"
2. **Status panel** — "Current State: Recording" / "Current State: Idle"
3. **Live panel** — "VAD Mode: ● Active" / "VAD Mode: ○ Inactive"

Three places for the same single piece of information.

### Issue C: Settings Split Across Two Panels

User-configurable settings are split between:
- **Configuration panel**: Server URL, Device, Language, Text Filters (lowercase, punctuation)
- **Live panel**: Progressive Typing, Auto-correction

There's no logical reason for this split. A user looking for "settings" has to check two different panels.

### Issue D: Live Panel is Overloaded

The Live Transcription panel tries to do too many things at once:
1. Show VAD status (redundant with header)
2. Show live transcription text (the actual useful content)
3. Show settings toggles (progressive typing, auto-correction)
4. Show stats (latency, segments processed)

On a small terminal (say 24 lines), with 9 lines of chrome, you only have ~15 lines for content. The Live panel uses ~6 lines for non-transcription UI (VAD status, settings, stats section headers, blank lines), leaving only ~9 lines for actual transcription text — the thing you're actually there to see.

### Issue E: Excessive Blank Lines / Wasted Vertical Space

Every panel uses generous `Line::from("")` spacers. In the Live panel alone, there are blank lines:
- Before VAD status (line 1)
- After VAD status (line 3)
- Before "Transcription:" header (line 4)
- After transcription text
- Before "Settings:" header (2 blank lines!)
- Before "Stats:" header (2 blank lines!)

This wastes significant vertical space in a terminal UI where every line matters.

### Issue F: The "Recording" Feature (Push-to-Talk) Has No Dedicated Panel

The legacy push-to-talk recording mode (toggled with Space on non-Live panels) doesn't have its own UI — it just changes a line in the Status panel and adds logs. You can't see audio levels, recording duration in a prominent way, or the transcription result. It's essentially a ghost feature in the TUI.

### Issue G: Logs Panel Doesn't Auto-Scroll Visually

While the code tracks `selected_log` and auto-advances it when viewing the last entry, the `List` widget renders all items from the beginning with the selected one highlighted. The selected item isn't scrolled into view — on a long log list, the highlighted item could be off-screen. The `List` widget needs a `ListState` with `.offset()` or `.select()` to actually scroll.

### Issue H: Footer Keybinding Help is Redundant with In-Panel Hints

The Configuration panel shows inline hints like `[e]`, `[d]`, `[f]`, `[p]` next to each field AND has a help line at the bottom of the panel content AND shows the same keybindings in the footer bar. That's three layers of keybinding documentation for the same panel.

### Issue I: Space Key Behavior is Context-Dependent and Confusing

- On the **Live panel**: Space toggles VAD mode
- On **any other panel**: Space toggles push-to-talk recording
- The `v` key **always** toggles VAD mode regardless of panel

This means Space does different things depending on which tab you're on, which is surprising behavior.

### Issue J: No Visual Feedback for Speech Detection

When VAD detects speech (`is_speaking = true`), the header changes from ◉ (green) to ● (yellow), but the Live panel doesn't show any real-time audio level or visual indicator. Users can't tell if the microphone is picking up their voice until a transcription appears.

---

## 3. Improvement Proposals

### Proposal 1: Eliminate the Status Panel — Merge Into Configuration

**Rationale**: The Status panel shows a strict subset of what Configuration already shows. The one unique field ("Current State") is already in the header.

**Action**:
- Remove the Status tab entirely
- Tabs become: `Configuration | Logs | Live`
- The Configuration panel already shows everything Status showed, plus it's editable
- Default tab becomes Configuration (or Live, since that's the primary use case)

**Impact**: One fewer tab to navigate, zero information loss.

### Proposal 2: Consolidate All Settings Into Configuration Panel

**Rationale**: Progressive Typing and Auto-correction toggles currently live on the Live panel but they're settings, not live transcription content.

**Action**:
- Move Progressive Typing and Auto-correction toggles to the Configuration panel under a "Typing" section
- Add `t` and `a` as keybindings on the Configuration panel
- The Live panel becomes purely about showing live transcription output

### Proposal 3: Simplify the Live Panel to Focus on Transcription

After moving settings out (Proposal 2), the Live panel should show:

```
┌─ Live Transcription ──────────────────────────┐
│                                                │
│  Hello this is a test of the transcription     │
│  system working in real time and the text      │
│  appears as I speak...                         │
│  [uncommitted text in gray]                    │
│                                                │
│                                                │
│                          Latency: 245ms  #12   │
└────────────────────────────────────────────────┘
```

- VAD status is already in the header — don't repeat it
- Stats (latency + segment count) go in the bottom-right corner as a subtle one-line overlay, not a full section
- Remove all the blank-line padding
- Maximize space for the actual transcription text

### Proposal 4: Fix Log Scrolling with ListState

**Action**: Use ratatui's `ListState` with `.select(Some(app.selected_log))` so the list widget actually scrolls to keep the selected item visible, instead of just highlighting it.

### Proposal 5: Make Space Key Consistent

**Option A (Recommended)**: Space always toggles VAD mode (the primary use case). Remove push-to-talk recording from the TUI entirely — it's a daemon feature accessible via `ears toggle` from a keybind.

**Option B**: Space always toggles VAD. Push-to-talk gets a different key (e.g., `r` for record).

### Proposal 6: Remove Redundant In-Panel Keybinding Hints

The footer already shows context-sensitive keybindings. Remove the inline hints like `[e]`, `[d]`, `[Shift+L to cycle]` from within the Configuration panel content to reduce visual clutter. The footer is sufficient.

Alternatively, keep the inline hints but remove the panel-bottom help line to avoid triple-documentation.

### Proposal 7: Reduce Tab Count — Consider Two-Panel Layout

With the Status panel removed, we have 3 tabs. An alternative approach:

**Two-panel layout (no tabs):**
```
┌─ ears v0.5.0 ─── ● VAD Active ───────────────┐
│                                                │
│  [Transcription area - takes most of screen]   │
│  Hello this is live transcription text...      │
│                                                │
│                          Latency: 245ms  #12   │
├────────────────────────────────────────────────┤
│ Server: http://... │ Model: whisper-large-v3   │
│ Device: Built-in   │ Lang: auto │ [x] Lower   │
├────────────────────────────────────────────────┤
│ [Space] VAD  [c] Config  [L] Lang  [q] Quit   │
└────────────────────────────────────────────────┘
```

- Top: Live transcription (the primary content)
- Bottom strip: Key config values at a glance
- Logs accessible via `:logs` command or `g` key (opens overlay/replaces content)
- This eliminates tab navigation entirely for the common case

### Proposal 8: Add Audio Level Indicator

When VAD is active, show a simple audio level bar in the header or near the transcription area:

```
│ ▸ VAD Active ████░░░░░░ │
```

This gives immediate feedback that the mic is working and picking up sound, before any transcription appears.

---

## 4. Priority Ranking

| Priority | Proposal | Effort | Impact |
|----------|----------|--------|--------|
| **P0** | #1 — Remove Status panel (redundant) | Low | High — removes confusion |
| **P0** | #4 — Fix log scrolling | Low | Medium — actual bug |
| **P1** | #2 — Consolidate settings | Low | Medium — cleaner mental model |
| **P1** | #3 — Simplify Live panel | Medium | High — better primary UX |
| **P1** | #5 — Consistent Space key | Low | Medium — less surprise |
| **P2** | #6 — Remove redundant hints | Low | Low — cosmetic |
| **P2** | #7 — Two-panel layout | High | High — but risky redesign |
| **P2** | #8 — Audio level indicator | Medium | Medium — nice-to-have |

---

## 5. Summary

The biggest issue with the current TUI is **redundancy**: the same information (state, server, device, model, language) appears in multiple places across multiple panels. The Status panel is almost entirely duplicated by the Configuration panel and the header. Settings are split illogically between Configuration and Live panels. The result is a 4-tab interface where 2 tabs would suffice.

The recommended minimal changes are:
1. **Kill the Status tab** (zero information loss)
2. **Move settings toggles to Configuration** (consolidate all config in one place)
3. **Dedicate the Live panel purely to transcription** (maximize the useful content area)
4. **Fix the log List widget to actually scroll** (bug fix)

These 4 changes would transform the TUI from "4 cluttered panels with lots of overlap" to "2 focused panels: config and live transcription" — a much cleaner experience.
