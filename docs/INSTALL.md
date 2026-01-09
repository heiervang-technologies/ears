# Installation & Usage Guide

This guide covers installing and using ears, a production-grade speech recognition daemon for Linux.

## Table of Contents

- [Quick Start](#quick-start)
- [System Requirements](#system-requirements)
- [Installing Dependencies](#installing-dependencies)
- [Setting Up Whisper.cpp Server](#setting-up-whispercpp-server)
- [Installing ears](#installing-ears)
- [Configuration](#configuration)
- [Usage](#usage)
- [Keyboard Shortcuts](#keyboard-shortcuts)
- [Troubleshooting](#troubleshooting)

## Quick Start

For experienced users who want to get started quickly:

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt install pipewire ydotool libnotify-bin pulseaudio-utils fzf jq curl

# Set up whisper.cpp server
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp && make server
bash ./models/download-ggml-model.sh base.en
./server -m models/ggml-base.en.bin -p 8178 &

# Start ydotool daemon
ydotoold &

# Install ears (Rust version)
git clone https://github.com/heiervang-technologies/ears
cd ears
cargo build --release
cargo install --path .

# Configure
ears --server http://localhost:8178
ears --select  # Choose your microphone

# Bind to keyboard shortcut and use!
```

## System Requirements

### Operating System

- **Linux** with one of the following:
  - Ubuntu 20.04+ (or derivative)
  - Debian 11+
  - Arch Linux
  - Fedora 34+
  - Any modern Linux distribution with PipeWire support

### Audio System

- **PipeWire** audio server (required)
  - Most modern Linux distributions use PipeWire by default
  - Check: `pactl info | grep "Server Name"` should show "PulseAudio (on PipeWire)"

### Hardware

- **Microphone** (any PipeWire-compatible audio input device)
- **Network** access to whisper.cpp server (can be localhost)
- **Memory**: 100MB RAM for ears daemon
- **Storage**: ~50MB for installation

### Optional but Recommended

- **GPU** with CUDA support for fast transcription (NVIDIA)
- **10GB+ free space** for whisper.cpp models

## Installing Dependencies

### Ubuntu / Debian / Pop!_OS / Linux Mint

```bash
sudo apt update
sudo apt install -y \
  pipewire \
  pipewire-audio-client-libraries \
  ydotool \
  libnotify-bin \
  pulseaudio-utils \
  fzf \
  jq \
  curl \
  pkg-config \
  libssl-dev
```

### Arch Linux / Manjaro

```bash
sudo pacman -S \
  pipewire \
  pipewire-pulse \
  ydotool \
  libnotify \
  pulseaudio \
  fzf \
  jq \
  curl \
  openssl \
  pkg-config
```

### Fedora / RHEL / CentOS Stream

```bash
sudo dnf install -y \
  pipewire \
  pipewire-pulseaudio \
  ydotool \
  libnotify \
  pulseaudio-utils \
  fzf \
  jq \
  curl \
  openssl-devel \
  pkg-config
```

### Rust Toolchain

If you don't have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

## Setting Up Whisper.cpp Server

The whisper.cpp server handles the actual speech-to-text transcription.

### 1. Clone and Build

```bash
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
```

**With GPU acceleration (NVIDIA):**
```bash
make server WHISPER_CUDA=1
```

**Without GPU (CPU only):**
```bash
make server
```

### 2. Download a Model

Whisper offers several model sizes with different speed/accuracy trade-offs:

| Model | Size | RAM | Accuracy | Speed (RTX 3090) |
|-------|------|-----|----------|------------------|
| tiny.en | 75 MB | ~1 GB | Good | ~10x realtime |
| base.en | 142 MB | ~1 GB | Better | ~7x realtime |
| small.en | 466 MB | ~2 GB | Great | ~4x realtime |
| medium.en | 1.5 GB | ~5 GB | Excellent | ~2x realtime |
| large | 2.9 GB | ~10 GB | Best | ~1x realtime |

**Recommended**: Start with `base.en` for a good balance:

```bash
bash ./models/download-ggml-model.sh base.en
```

For better accuracy, use `small.en`:

```bash
bash ./models/download-ggml-model.sh small.en
```

### 3. Start the Server

**With GPU:**
```bash
./server -m models/ggml-base.en.bin -p 8178 --gpu
```

**Without GPU:**
```bash
./server -m models/ggml-base.en.bin -p 8178
```

**Run in background:**
```bash
nohup ./server -m models/ggml-base.en.bin -p 8178 --gpu > /tmp/whisper.log 2>&1 &
```

**Verify it's running:**
```bash
curl -sf http://localhost:8178/health && echo "Server is running!"
```

### 4. (Optional) Create a Systemd Service

To automatically start whisper.cpp on boot:

```bash
mkdir -p ~/.config/systemd/user
cat > ~/.config/systemd/user/whisper-server.service <<EOF
[Unit]
Description=Whisper.cpp Server
After=network.target

[Service]
Type=simple
ExecStart=/path/to/whisper.cpp/server -m /path/to/models/ggml-base.en.bin -p 8178 --gpu
Restart=on-failure

[Install]
WantedBy=default.target
EOF

systemctl --user enable whisper-server
systemctl --user start whisper-server
```

## Setting Up ydotool

ydotool simulates keyboard input to type the transcribed text.

### 1. Start the Daemon

```bash
ydotoold &
```

### 2. (Optional) Enable as a Systemd Service

```bash
systemctl --user enable ydotool
systemctl --user start ydotool
```

### 3. Verify It Works

```bash
ydotool type "test"
```

You should see "test" typed into your active window.

### Troubleshooting ydotool

If ydotool doesn't work:

1. **Check the daemon is running:**
   ```bash
   pgrep ydotoold || echo "ydotool daemon not running!"
   ```

2. **Check socket permissions:**
   ```bash
   ls -la /tmp/.ydotool_socket
   ```

3. **On some systems, you may need to add yourself to the `input` group:**
   ```bash
   sudo usermod -aG input $USER
   # Log out and log back in
   ```

## Installing ears

### From Source (Recommended)

```bash
# Clone the repository
git clone https://github.com/heiervang-technologies/ears
cd ears

# Build release version
cargo build --release

# Install to ~/.cargo/bin/
cargo install --path .

# Verify installation
ears --help
```

### From Crates.io (Coming Soon)

```bash
cargo install ears
```

## Configuration

### Set Whisper Server URL

```bash
# Set server URL (default: http://localhost:8178)
ears --server http://localhost:8178

# View current server
ears --server
```

The configuration is stored in `~/.config/ears/config.toml`.

### Select Microphone

```bash
# List available audio input devices
ears --list

# Interactively select device (uses fzf)
ears --select

# Show currently selected device
ears --current
```

The selected device is stored in `~/.config/ears/config.toml`.

### Configuration File

You can also manually edit `~/.config/ears/config.toml`:

```toml
whisper_server = "http://localhost:8178"
device = "alsa_input.usb-Blue_Microphones_Yeti_Stereo_Microphone_REV8-00.analog-stereo"
```

## Usage

### Basic Workflow

ears works as a push-to-talk voice recognition tool:

1. **Press your keyboard shortcut** → Recording starts (you'll hear a beep)
2. **Speak your message**
3. **Press the shortcut again** → Recording stops, transcription happens
4. **Text is typed** into your active window

### Command-Line Interface

```bash
# Toggle recording/transcription (main use case)
ears

# Configuration commands
ears --list          # List audio devices
ears --select        # Select audio device interactively
ears --current       # Show current device
ears --server [URL]  # Show or set whisper server URL

# Information
ears --help          # Show help
ears --version       # Show version
```

### TUI Mode (Interactive Dashboard)

```bash
# Launch interactive TUI
ears --tui

# TUI shows:
# - Current status (Idle/Recording/Transcribing)
# - Recording duration
# - Recent transcriptions
# - Configuration
# - Logs
```

Keyboard shortcuts in TUI:
- `Space` - Toggle recording
- `q` - Quit
- `?` - Help

## Keyboard Shortcuts

To use ears effectively, bind it to a global keyboard shortcut.

### GNOME / Ubuntu / Pop!_OS

1. Open **Settings** → **Keyboard** → **Keyboard Shortcuts**
2. Scroll to bottom, click **"+"** to add custom shortcut
3. Set:
   - **Name**: ears
   - **Command**: `ears` (or full path: `/home/yourusername/.cargo/bin/ears`)
   - **Shortcut**: Press your preferred key combination (e.g., `Super+Shift+V`)

### KDE Plasma

1. Open **System Settings** → **Shortcuts**
2. Select **Custom Shortcuts**
3. **Edit** → **New** → **Global Shortcut** → **Command/URL**
4. Set:
   - **Trigger**: Your preferred key combination
   - **Action**: `ears`

### i3 Window Manager

Add to `~/.config/i3/config`:

```bash
bindsym $mod+Shift+v exec ears
```

Reload i3: `$mod+Shift+r`

### Sway (Wayland)

Add to `~/.config/sway/config`:

```bash
bindsym $mod+Shift+v exec ears
```

Reload: `swaymsg reload`

### XFCE

1. **Settings** → **Keyboard** → **Application Shortcuts**
2. Click **Add**
3. Enter command: `ears`
4. Press your desired shortcut

## Troubleshooting

### "Whisper server not running" Error

**Symptoms**: Notification says "Whisper server not running!" when trying to transcribe.

**Solutions**:

1. **Check if server is running:**
   ```bash
   curl -sf http://localhost:8178/health && echo "OK" || echo "Server not responding"
   ```

2. **Verify server URL:**
   ```bash
   ears --server
   ```

3. **Check whisper.cpp logs:**
   ```bash
   # If running in background
   tail -f /tmp/whisper.log

   # Or check process
   pgrep -a server | grep whisper
   ```

4. **Restart server:**
   ```bash
   cd /path/to/whisper.cpp
   ./server -m models/ggml-base.en.bin -p 8178 --gpu
   ```

### "No active recording" Error

**Symptoms**: Pressing the shortcut a second time says "No active recording".

**Solutions**:

1. **Recording may have timed out** (default: 2 minutes max)
   - Check state: `ls -la $XDG_RUNTIME_DIR/ears/`

2. **Process might have crashed:**
   ```bash
   # Check for stale pw-record processes
   pgrep pw-record
   ```

3. **Check logs:**
   ```bash
   tail -20 $XDG_RUNTIME_DIR/ears/ears.log
   ```

### Text Isn't Being Typed

**Symptoms**: Transcription succeeds but text doesn't appear in the active window.

**Solutions**:

1. **Check ydotool daemon:**
   ```bash
   pgrep ydotoold || echo "Daemon not running!"

   # Start it
   ydotoold &
   ```

2. **Test ydotool manually:**
   ```bash
   ydotool type "test message"
   ```

3. **Check permissions:**
   ```bash
   # Add yourself to input group if needed
   sudo usermod -aG input $USER
   # Log out and back in
   ```

4. **Try running ears from terminal** to see any error messages:
   ```bash
   ears
   # Press shortcut, speak, press again
   # Watch for errors
   ```

### Wrong Microphone Being Used

**Symptoms**: Recording doesn't pick up your voice, or picks up wrong audio source.

**Solutions**:

1. **List all devices:**
   ```bash
   ears --list
   ```

2. **Select correct device:**
   ```bash
   ears --select
   # Use arrow keys to choose, Enter to select
   ```

3. **Verify selection:**
   ```bash
   ears --current
   ```

4. **Test recording manually:**
   ```bash
   pw-record --target YOUR_DEVICE test.wav
   # Speak for a few seconds, then Ctrl+C
   paplay test.wav  # Listen to playback
   ```

### Poor Transcription Quality

**Symptoms**: Transcriptions are inaccurate or contain gibberish.

**Solutions**:

1. **Use a better whisper model:**
   ```bash
   cd /path/to/whisper.cpp
   bash ./models/download-ggml-model.sh small.en
   # Update server to use new model
   ./server -m models/ggml-small.en.bin -p 8178 --gpu
   ```

2. **Check microphone audio levels** in system settings
   - Ensure input isn't too quiet or clipping

3. **Test audio quality:**
   ```bash
   pw-record --target YOUR_DEVICE test.wav
   # Speak clearly, stop after a few seconds
   paplay test.wav
   # Should sound clear
   ```

4. **Reduce background noise**
   - Use a better microphone
   - Move closer to microphone
   - Use push-to-talk in quiet environment

### Transcription is Too Slow

**Symptoms**: Long delay between stopping recording and text appearing.

**Solutions**:

1. **Use GPU acceleration:**
   ```bash
   # Rebuild whisper.cpp with CUDA
   cd /path/to/whisper.cpp
   make server WHISPER_CUDA=1
   ./server -m models/ggml-base.en.bin -p 8178 --gpu
   ```

2. **Use a smaller/faster model:**
   - `tiny.en` - Fastest, less accurate
   - `base.en` - Good balance (recommended)
   - `small.en` - Better accuracy, slower

3. **Check server performance:**
   ```bash
   # Monitor whisper.cpp server logs for inference time
   # Should be < 2 seconds for 10 second audio on GPU
   ```

### Permission Denied Errors

**Symptoms**: Errors about unable to create files or directories.

**Solutions**:

1. **Check config directory permissions:**
   ```bash
   ls -la ~/.config/ears/
   # Should be owned by you
   ```

2. **Check runtime directory:**
   ```bash
   ls -la $XDG_RUNTIME_DIR/ears/
   ```

3. **Recreate directories:**
   ```bash
   rm -rf ~/.config/ears $XDG_RUNTIME_DIR/ears
   ears --server http://localhost:8178  # Recreates config
   ```

## Advanced Topics

### Using a Remote Whisper Server

You can run whisper.cpp on a different machine (e.g., a server with a powerful GPU):

1. **On the server**, start whisper.cpp with network binding:
   ```bash
   ./server -m models/ggml-base.en.bin -p 8178 --host 0.0.0.0 --gpu
   ```

2. **On your client**, configure ears:
   ```bash
   ears --server http://server-ip:8178
   ```

**Security note**: Only do this on a trusted network! Consider using SSH tunneling for secure remote access:

```bash
# On your local machine
ssh -L 8178:localhost:8178 user@remote-server

# Configure ears to use localhost (it will tunnel through SSH)
ears --server http://localhost:8178
```

### Custom Audio Feedback Sounds

You can customize the beep sounds:

```bash
mkdir -p ~/.local/share/ears-sounds/
cp your-start-sound.wav ~/.local/share/ears-sounds/start.wav
cp your-done-sound.wav ~/.local/share/ears-sounds/done.wav
cp your-error-sound.wav ~/.local/share/ears-sounds/bell.wav
```

### Integration with Other Tools

**Copy to clipboard instead of typing:**

Modify the ears source to use `wl-copy` (Wayland) or `xclip` (X11) instead of ydotool.

**Post-process transcriptions:**

You can create wrapper scripts that process the transcription before typing:

```bash
#!/bin/bash
# ears-wrapper.sh
text=$(ears --print-only)  # hypothetical flag
processed=$(echo "$text" | your-processing-command)
echo "$processed" | ydotool type --file -
```

## Getting Help

- **GitHub Issues**: [https://github.com/heiervang-technologies/ears/issues](https://github.com/heiervang-technologies/ears/issues)
- **Documentation**: Check `docs/` directory in the repository
- **Logs**: `$XDG_RUNTIME_DIR/ears/ears.log` and whisper.cpp server logs

## Next Steps

- Read [ARCHITECTURE.md](ARCHITECTURE.md) to understand how ears works
- Read [CONTRIBUTING.md](CONTRIBUTING.md) if you want to contribute
- Check the [API documentation](https://docs.rs/ears) for library usage
