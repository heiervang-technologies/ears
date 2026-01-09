# User Guide

This guide covers installation, configuration, and day-to-day usage of `ears` - a production-grade speech recognition daemon for Linux.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [Usage](#usage)
- [Troubleshooting](#troubleshooting)

## Installation

### Prerequisites

`ears` requires the following system components:

**Required:**
- Linux with PipeWire audio system
- [whisper.cpp server](https://github.com/ggerganov/whisper.cpp) running
- `ydotool` for text input automation

**Optional (for full functionality):**
- `notify-send` for desktop notifications
- `paplay` for audio feedback
- `fzf` for interactive device selection

### Installing System Dependencies

#### Ubuntu/Debian
```bash
sudo apt install pipewire ydotool libnotify-bin pulseaudio-utils fzf
```

#### Arch Linux
```bash
sudo pacman -S pipewire ydotool libnotify pulseaudio fzf
```

#### Fedora
```bash
sudo dnf install pipewire ydotool libnotify pulseaudio-utils fzf
```

### Setting up whisper.cpp Server

1. **Clone and build whisper.cpp:**
   ```bash
   git clone https://github.com/ggerganov/whisper.cpp
   cd whisper.cpp
   make server
   ```

2. **Download a model:**
   ```bash
   bash ./models/download-ggml-model.sh base.en
   ```
   
   Available models (larger = more accurate but slower):
   - `tiny.en` - Fastest, least accurate (75 MB)
   - `base.en` - Good balance (142 MB) **← Recommended**
   - `small.en` - Better accuracy (466 MB)
   - `medium.en` - High accuracy (1.5 GB)
   - `large` - Best accuracy, multilingual (2.9 GB)

3. **Start the server:**
   ```bash
   ./server -m models/ggml-base.en.bin -p 8178
   ```

4. **For GPU acceleration (NVIDIA):**
   ```bash
   make server WHISPER_CUDA=1
   ./server -m models/ggml-base.en.bin -p 8178 --gpu
   ```

### Setting up ydotool

ydotool requires a background daemon:

```bash
# Start the daemon
ydotoold &

# Or enable as a systemd user service (recommended)
systemctl --user enable ydotool
systemctl --user start ydotool
```

Verify it's running:
```bash
pgrep ydotoold  # Should return a process ID
```

### Installing ears

#### From Source

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
cargo build --release
sudo cp target/release/ears /usr/local/bin/
```

#### Using install.sh

```bash
./install.sh
```

This builds and installs to `~/.local/bin/ears`. Ensure `~/.local/bin` is in your PATH:
```bash
export PATH="$HOME/.local/bin:$PATH"
```

Add to your `~/.bashrc` or `~/.zshrc` to make it permanent.

## Quick Start

### 1. Configure whisper.cpp Server

Tell `ears` where your whisper.cpp server is running:

```bash
ears server http://localhost:8178
```

Verify the configuration:
```bash
ears server
# Output: http://localhost:8178
```

### 2. Select Your Microphone

List available audio devices:
```bash
ears list
```

Select your microphone interactively:
```bash
ears select
```

Or show the currently configured device:
```bash
ears current
```

### 3. Set Up a Keyboard Shortcut

#### GNOME/Ubuntu

1. Open **Settings** → **Keyboard** → **Keyboard Shortcuts**
2. Scroll down and click **Custom Shortcuts**
3. Click the **+** button
4. Fill in:
   - **Name:** ears
   - **Command:** `ears` (or full path: `/usr/local/bin/ears`)
   - **Shortcut:** Press your preferred key combo (e.g., `Super+Shift+V`)

#### KDE Plasma

1. Open **System Settings** → **Shortcuts**
2. Click **Custom Shortcuts** in the sidebar
3. Right-click in the right pane → **New** → **Global Shortcut** → **Command/URL**
4. **Trigger** tab: Set your key combo
5. **Action** tab: Enter `ears`

#### i3/Sway

Add to your config file (`~/.config/i3/config` or `~/.config/sway/config`):

```
bindsym $mod+Shift+v exec ears
```

Reload your config: `$mod+Shift+c`

### 4. Use It!

1. **Press your shortcut** - Recording starts (beep sound)
2. **Speak your message**
3. **Press shortcut again** - Recording stops, transcription happens
4. **Text appears** in your active window

## Configuration

### Configuration Files

`ears` stores configuration in standard XDG directories:

| File | Purpose | Example |
|------|---------|---------|
| `~/.config/ears/server` | whisper.cpp server URL | `http://localhost:8178` |
| `~/.config/ears/device` | Selected audio device name | `alsa_input.usb-Blue_Microphones_Yeti_Stereo_Microphone` |

### Environment Variables

You can override configuration with environment variables:

| Variable | Purpose | Example |
|----------|---------|---------|
| `EARS_SERVER` | whisper.cpp server URL | `http://192.168.1.100:8178` |
| `EARS_DEVICE` | Audio input device | `alsa_input.usb-...` |

Example:
```bash
EARS_SERVER=http://192.168.1.100:8178 ears
```

### Runtime State

During operation, `ears` creates temporary files in `$XDG_RUNTIME_DIR/ears/`:

| File | Purpose |
|------|---------|
| `lock` | Prevents concurrent instances |
| `state` | Current state (Idle/Recording/Transcribing) |
| `recording.pid` | PID of the recording process |
| `recording.wav` | Temporary audio file |

These are automatically cleaned up when you log out.

### Custom Audio Feedback

Place custom WAV files in `~/.local/share/ears-sounds/`:

- `start.wav` - Played when recording starts
- `done.wav` - Played when transcription completes
- `bell.wav` - Played on errors

Falls back to system sounds if not found.

## Usage

### Command-Line Interface

```
ears [COMMAND]

Commands:
  server [URL]   Show or set whisper server URL
  select         Select audio device with fzf
  list           List available audio devices
  current        Show current device
  help           Show help information

Without a command: Toggle recording/transcription
```

### CLI Examples

```bash
# Show current server configuration
ears server

# Change server URL
ears server http://192.168.1.100:8178

# List all audio input devices
ears list

# Interactively select a device
ears select

# Show currently configured device
ears current

# Toggle recording (normal usage)
ears
```

### How the Toggle Works

The main mode of operation is the toggle behavior (running `ears` with no arguments):

1. **First press:**
   - Checks if whisper.cpp server is available
   - Creates state directory if needed
   - Acquires lock file
   - Starts recording with `pw-record`
   - Saves state as "Recording"
   - Plays start sound
   
2. **Second press:**
   - Reads current state
   - Stops the recording process
   - Changes state to "Transcribing"
   - Waits briefly for file to be written
   - Sends audio to whisper.cpp server
   - Filters out silence artifacts
   - Types the transcription using ydotool
   - Cleans up temporary files
   - Returns to "Idle" state
   - Plays done sound

### Recording Format

Audio is recorded with these parameters (optimized for whisper.cpp):

- **Sample rate:** 16 kHz
- **Channels:** 1 (mono)
- **Format:** Signed 16-bit PCM (s16le)
- **Container:** WAV

### Timeout Protection

Recordings automatically stop after **2 minutes** to prevent runaway recordings from:
- Forgotten recording sessions
- Keyboard shortcut misfires
- Application crashes

If timeout occurs, you'll get a notification and the state resets to Idle.

### Silence Artifact Filtering

whisper.cpp sometimes generates false transcriptions when given silence. `ears` filters these out:

- Empty strings
- "Thank you."
- "Thanks for watching."
- Other common silence artifacts

## Troubleshooting

### "Whisper server not running!" or Connection Errors

**Symptoms:** Notification says server isn't available

**Solutions:**
1. Check if whisper.cpp server is running:
   ```bash
   curl -sf http://localhost:8178/health || echo "Server not responding"
   ```

2. Verify server URL configuration:
   ```bash
   ears server
   ```

3. Check whisper.cpp server logs for errors

4. If using a remote server, check network connectivity:
   ```bash
   ping 192.168.1.100
   curl http://192.168.1.100:8178/health
   ```

### "No active recording" When Trying to Stop

**Symptoms:** Second press says no recording is active

**Possible Causes:**
1. **Recording timed out** (2 minute limit)
2. **Lock file stale** from crashed previous session
3. **PID file missing** or process was killed externally

**Solutions:**
1. Check runtime state:
   ```bash
   ls -la $XDG_RUNTIME_DIR/ears/
   cat $XDG_RUNTIME_DIR/ears/state
   ```

2. Clean up stale state:
   ```bash
   rm -rf $XDG_RUNTIME_DIR/ears/
   ```

3. Try again

### Wrong Microphone Being Used

**Symptoms:** Recording doesn't capture your voice

**Solutions:**
1. List available devices:
   ```bash
   ears list
   ```

2. Select the correct device:
   ```bash
   ears select
   ```

3. Verify selection:
   ```bash
   ears current
   ```

4. Test the device manually:
   ```bash
   pw-record --target YOUR_DEVICE_NAME test.wav
   # Speak for a few seconds, then Ctrl+C
   paplay test.wav  # Play it back
   ```

### Text Not Being Typed

**Symptoms:** Transcription succeeds but text doesn't appear

**Solutions:**
1. Check if ydotool daemon is running:
   ```bash
   pgrep ydotoold || echo "ydotool daemon not running"
   ```

2. Start ydotool daemon:
   ```bash
   ydotoold &
   # Or
   systemctl --user start ydotool
   ```

3. Test ydotool manually:
   ```bash
   ydotool type "test message"
   ```

4. Check permissions (ydotool needs access to `/dev/uinput`)

### Poor Transcription Quality

**Symptoms:** Transcriptions are inaccurate or garbled

**Solutions:**
1. **Check microphone input level** in system sound settings
   - Too quiet: Increase gain/volume
   - Clipping: Reduce gain/volume

2. **Test recording quality:**
   ```bash
   pw-record --target YOUR_DEVICE test.wav
   # Speak clearly for 5 seconds, then Ctrl+C
   paplay test.wav
   ```

3. **Try a larger whisper model:**
   - `base.en` (default) - Good for most use
   - `small.en` - Better accuracy
   - `medium.en` - High accuracy

4. **Check for background noise**
   - Use push-to-talk deliberately
   - Consider noise suppression in PipeWire settings

5. **Speak clearly and at a moderate pace**

### Transcription Takes Too Long

**Symptoms:** Long delay between stopping recording and text appearing

**Solutions:**
1. **Use GPU acceleration** if available:
   ```bash
   # Build with CUDA support
   cd whisper.cpp
   make clean
   make server WHISPER_CUDA=1
   
   # Run with GPU
   ./server -m models/ggml-base.en.bin --gpu -p 8178
   ```

2. **Use a smaller model** (faster but less accurate):
   ```bash
   # Download tiny model
   bash ./models/download-ggml-model.sh tiny.en
   
   # Use it
   ./server -m models/ggml-tiny.en.bin -p 8178
   ```

3. **Check whisper.cpp server load**
   - Are multiple clients using it?
   - Check CPU/GPU usage during transcription

### Audio Feedback Not Working

**Symptoms:** No beep sounds when recording starts/stops

**Solutions:**
1. Check if `paplay` is installed:
   ```bash
   which paplay || echo "Install pulseaudio-utils"
   ```

2. Test system sounds:
   ```bash
   paplay /usr/share/sounds/freedesktop/stereo/bell.wav
   ```

3. Add custom sounds:
   ```bash
   mkdir -p ~/.local/share/ears-sounds
   # Copy your WAV files there
   ```

4. Check PulseAudio/PipeWire audio output

### Desktop Notifications Not Showing

**Symptoms:** No error notifications appear

**Solutions:**
1. Check if `notify-send` is available:
   ```bash
   which notify-send || echo "Install libnotify"
   ```

2. Test notifications:
   ```bash
   notify-send "Test" "This is a test notification"
   ```

3. Check notification settings in your desktop environment

### Permission Denied Errors

**Symptoms:** Errors about permissions when recording or typing

**Solutions:**
1. **For PipeWire:** Ensure you're in the `audio` group
   ```bash
   groups | grep audio || echo "Not in audio group"
   sudo usermod -aG audio $USER
   # Log out and back in
   ```

2. **For ydotool:** Check uinput permissions
   ```bash
   ls -l /dev/uinput
   # Should be accessible to your user or group
   ```

### Lock File Errors

**Symptoms:** "Could not acquire lock" errors

**Solutions:**
1. Check if another instance is running:
   ```bash
   ps aux | grep ears
   ```

2. Remove stale lock:
   ```bash
   rm -f $XDG_RUNTIME_DIR/ears/lock
   ```

## Advanced Usage

### Using a Remote whisper.cpp Server

You can run whisper.cpp on a more powerful machine and connect to it:

**On the server machine:**
```bash
# Bind to all interfaces
./server -m models/ggml-base.en.bin --host 0.0.0.0 -p 8178
```

**On your laptop:**
```bash
ears server http://192.168.1.100:8178
```

**Security note:** whisper.cpp server has no authentication. Use within trusted networks only.

### Scripting with ears

You can use `ears` in scripts:

```bash
#!/bin/bash
# Record a voice note and save transcription

# Start recording
ears

# Speak your message...

# Stop and transcribe (text is typed automatically)
ears

# The transcription was typed into the active window
# To capture it, you'd need to modify ears to output to stdout instead
```

### Integration with Text Editors

For use with vim, emacs, or other editors:

1. Set up keyboard shortcut as normal
2. Focus your editor
3. Enter insert mode (vim) or position cursor
4. Press shortcut, speak, press again
5. Text appears at cursor position

### Multiple Microphone Setups

If you use different microphones in different situations:

```bash
# Work setup
ears select  # Choose USB headset
# ... use ears normally ...

# Podcast setup
ears select  # Choose XLR microphone
# ... use ears normally ...
```

Configuration is persistent until changed.

## Performance Notes

- **Recording:** Minimal CPU usage (PipeWire handles it)
- **Transcription:** Depends on whisper.cpp server performance
  - CPU: Slow but works (base.en model: ~5-10s for 30s audio)
  - GPU: Much faster (base.en model: ~0.5-2s for 30s audio)
- **State management:** Effectively instant (lock files are very fast)
- **Text input:** Fast (ydotool is efficient)
- **Memory:** Low (~5-10 MB for main process + recording buffer)

## Privacy and Security

- **Audio storage:** Temporary files in `$XDG_RUNTIME_DIR` (cleared on logout)
- **Network:** Audio sent to whisper.cpp server only
- **No cloud:** All processing is local (assuming local whisper.cpp)
- **No telemetry:** `ears` doesn't phone home
- **No persistent storage:** No recordings are kept after transcription

## Next Steps

- **API Documentation:** See [api.md](api.md) for using ears as a library
- **Architecture:** See [architecture.md](architecture.md) for system design details
- **Contributing:** See [architecture.md](architecture.md#contributing) for development guide

## Getting Help

- **Issues:** [GitHub Issues](https://github.com/heiervang-technologies/ears/issues)
- **Discussions:** [GitHub Discussions](https://github.com/heiervang-technologies/ears/discussions)
