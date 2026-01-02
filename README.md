# ears

A production-grade speech recognition daemon for Linux that integrates whisper.cpp with your desktop workflow.

## Features

- **Push-to-talk interface**: Press once to start recording, press again to transcribe
- **Whisper.cpp integration**: Leverages GPU-accelerated whisper.cpp server
- **PipeWire audio**: Native support for modern Linux audio stack
- **Configurable devices**: Easy microphone selection with fzf
- **Audio feedback**: Beep sounds for start/stop confirmation
- **Desktop notifications**: System notifications for errors and status
- **Direct text input**: Automatically types transcribed text using ydotool
- **State management**: Proper locking and cleanup of recording sessions
- **Timeout protection**: Automatically stops runaway recordings after 2 minutes

## Architecture

```
┌─────────────────┐
│  Keyboard       │
│  Shortcut       │
└────────┬────────┘
         │
         ▼
┌─────────────────┐     ┌──────────────┐     ┌─────────────┐
│  ears daemon    │────▶│  PipeWire    │────▶│ Audio       │
│  (toggle)       │     │  Recording   │     │ Device      │
└────────┬────────┘     └──────────────┘     └─────────────┘
         │
         │ (on second press)
         ▼
┌─────────────────┐     ┌──────────────┐     ┌─────────────┐
│  whisper.cpp    │────▶│  ydotool     │────▶│ Active      │
│  Server         │     │  (type text) │     │ Window      │
└─────────────────┘     └──────────────┘     └─────────────┘
```

## Prerequisites

- Linux with PipeWire audio system
- [whisper.cpp server](https://github.com/ggerganov/whisper.cpp) running
- `ydotool` for text input
- `notify-send` for notifications
- `paplay` for audio feedback
- `fzf` for device selection
- `jq` for JSON parsing
- `curl` for API communication

### Installing Dependencies

```bash
# Ubuntu/Debian
sudo apt install pipewire ydotool libnotify-bin pulseaudio-utils fzf jq curl

# Arch Linux
sudo pacman -S pipewire ydotool libnotify pulseaudio fzf jq curl
```

### Setting up whisper.cpp Server

1. Clone and build whisper.cpp:
```bash
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
make server
```

2. Download a model:
```bash
bash ./models/download-ggml-model.sh base.en
```

3. Start the server:
```bash
./server -m models/ggml-base.en.bin -p 8178
```

For GPU acceleration with CUDA:
```bash
make server WHISPER_CUDA=1
./server -m models/ggml-base.en.bin -p 8178 --gpu
```

### Setting up ydotool

ydotool requires running as a background service:

```bash
# Start the daemon
ydotoold &

# Or enable as a systemd user service
systemctl --user enable ydotool
systemctl --user start ydotool
```

## Installation

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
./install.sh
```

This will:
- Install `ears` to `~/.local/bin/ears`
- Create config directory at `~/.config/ears`
- Create sounds directory at `~/.local/share/ears-sounds`

Make sure `~/.local/bin` is in your PATH:
```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Configuration

### Whisper Server

Set the whisper.cpp server URL:
```bash
ears --server http://localhost:8178
```

View current server:
```bash
ears --server
```

Config is stored in: `~/.config/ears/server`

### Microphone Device

List available devices:
```bash
ears --list
```

Select device interactively:
```bash
ears --select
```

Show current device:
```bash
ears --current
```

Config is stored in: `~/.config/ears/device`

## Usage

### Basic Operation

Bind a keyboard shortcut to run `ears` (no arguments). Then:

1. **Press shortcut once** - Starts recording (you'll hear a beep)
2. **Speak your message**
3. **Press shortcut again** - Stops recording and transcribes
4. **Text is typed** into your active window

### Keyboard Shortcut Setup

#### GNOME/Ubuntu
```bash
# Settings → Keyboard → Custom Shortcuts
# Add new shortcut:
#   Name: ears
#   Command: /home/yourusername/.local/bin/ears
#   Shortcut: Your preferred key combo
```

#### KDE Plasma
```bash
# System Settings → Shortcuts → Custom Shortcuts
# Edit → New → Global Shortcut → Command/URL
#   Trigger: Your preferred key combo
#   Action: /home/yourusername/.local/bin/ears
```

#### i3/Sway
```bash
# Add to config:
bindsym $mod+Shift+v exec ears
```

### Command-Line Options

```
Usage: ears [OPTION]

Without options: Toggle recording/transcription

Options:
  -s, --select       Select audio device with fzf
  -l, --list         List available audio devices
  -c, --current      Show current device
  --server [URL]     Show or set whisper server URL
  -h, --help         Show this help
```

## How It Works

### State Management

ears uses lock files and PID tracking to maintain state:
- **Lock file**: `$XDG_RUNTIME_DIR/ears/lock` - Prevents concurrent instances
- **PID file**: `$XDG_RUNTIME_DIR/ears/recording.pid` - Tracks recording process
- **Audio file**: `$XDG_RUNTIME_DIR/ears/recording.wav` - Temporary recording storage

### Audio Recording

- Records at 16kHz, mono, signed 16-bit PCM (whisper.cpp's preferred format)
- Uses PipeWire's `pw-record` with explicit device targeting
- 2-minute timeout prevents runaway recordings
- Cleans up stale processes automatically

### Transcription Flow

1. Stops the recording process
2. Waits 300ms for file to be fully written
3. Validates audio file exists and has content
4. POSTs audio to whisper.cpp server
5. Extracts text from JSON response
6. Filters out whisper.cpp silence artifacts ("Thank you.")
7. Types text using ydotool
8. Cleans up temporary files

### Noise Filtering

ears filters common whisper.cpp false positives:
- Empty transcriptions
- The phrase "Thank you." (common silence artifact)

## Custom Sounds

Place custom WAV files in `~/.local/share/ears-sounds/`:
- `start.wav` - Played when recording starts
- `done.wav` - Played when transcription completes
- `bell.wav` - Played on errors

Falls back to system sounds if custom sounds aren't found.

## Troubleshooting

### "Whisper server not running!"
- Ensure whisper.cpp server is running
- Check server URL: `ears --server`
- Test server: `curl http://localhost:8178/health`

### "No active recording"
- Recording may have timed out (2 minute limit)
- Check state: `ls $XDG_RUNTIME_DIR/ears/`
- View logs: `cat $XDG_RUNTIME_DIR/ears/debug.log`

### "Transcription failed"
- Check whisper.cpp server logs
- Verify audio file was created: `ls -lh $XDG_RUNTIME_DIR/ears/recording.wav`
- Test manually: `curl -X POST http://localhost:8178/inference -F "file=@/path/to/recording.wav" -F "response_format=json"`

### Text isn't being typed
- Ensure ydotool daemon is running: `pgrep ydotoold`
- Test ydotool: `ydotool type "test"`
- Check permissions (ydotool may need special setup)

### Wrong microphone being used
- List devices: `ears --list`
- Select correct device: `ears --select`
- Verify: `ears --current`

### Audio quality issues
- Check microphone input level in system settings
- Test with: `pw-record --target YOUR_DEVICE test.wav` (Ctrl+C after a few seconds)
- Play back: `paplay test.wav`

## Performance Notes

- Recording uses minimal CPU (PipeWire handles it)
- Transcription speed depends on whisper.cpp server (GPU recommended)
- State management is instant (lock files are very fast)
- Audio feedback is non-blocking (plays in background)

## Development

### Project Structure

```
ears/
├── bin/
│   └── ears           # Main executable script
├── sounds/            # Optional custom sound files
├── install.sh         # Installation script
├── README.md          # This file
├── CLAUDE.md          # Agent instructions
└── .github/           # GitHub workflows (from template)
```

### Running from Source

```bash
cd ears
./bin/ears
```

### Debugging

Enable debug logging by checking `$XDG_RUNTIME_DIR/ears/debug.log`:
```bash
tail -f $XDG_RUNTIME_DIR/ears/debug.log
```

### Testing

Test individual components:

```bash
# List devices
./bin/ears --list

# Test server connection
curl -sf http://localhost:8178/health

# Test recording (5 seconds)
timeout 5 pw-record --target YOUR_DEVICE test.wav

# Test transcription
curl -X POST http://localhost:8178/inference \
  -F "file=@test.wav" \
  -F "response_format=json" | jq
```

## Security Considerations

- Audio is sent to whisper.cpp server (defaults to localhost)
- Configure `--server` to point to your server only
- Audio files are stored temporarily in `$XDG_RUNTIME_DIR` (cleared on logout)
- No audio is saved permanently by default
- Lock file prevents multiple recording sessions

## License

MIT

## Credits

Originally developed as `asr` for personal use, now production-ready as `ears`.

Built with:
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) - Fast whisper inference
- [PipeWire](https://pipewire.org/) - Modern Linux audio
- [ydotool](https://github.com/ReimuNotMoe/ydotool) - Generic input automation
