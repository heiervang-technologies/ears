# Agent Instructions

This file provides context and instructions for AI agents working on the ears repository.

## Repository Overview

**ears** is a production-grade speech recognition daemon for Linux that integrates whisper.cpp with desktop workflows. It provides a push-to-talk interface that captures audio via PipeWire, transcribes it using a whisper.cpp server, and types the result into the active window.

### Key Components

- **bin/ears** - Main bash script (single executable)
- **install.sh** - Installation script
- **sounds/** - Optional custom audio feedback files
- **.github/** - GitHub workflows (inherited from template)

### Technology Stack

- **Shell**: Bash with strict error handling (`set -euo pipefail`)
- **Audio**: PipeWire (`pw-record`, `pw-cli`)
- **Transcription**: whisper.cpp server (REST API)
- **Input**: ydotool for typing transcribed text
- **UI**: Desktop notifications (`notify-send`) and audio feedback (`paplay`)
- **State**: Lock files and PID tracking in `$XDG_RUNTIME_DIR`

## Development Setup

### Prerequisites

```bash
# Install dependencies
sudo apt install pipewire ydotool libnotify-bin pulseaudio-utils fzf jq curl

# Set up whisper.cpp server
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
make server WHISPER_CUDA=1  # Or without CUDA
./models/download-ggml-model.sh base.en
./server -m models/ggml-base.en.bin -p 8178 --gpu
```

### Testing the Script

```bash
# Run from source
cd ears
./bin/ears --help

# Test device listing
./bin/ears --list

# Test with actual recording (requires whisper.cpp server running)
./bin/ears  # Press once to start, again to stop
```

## Code Style

### Bash Best Practices

- Use strict mode: `set -euo pipefail`
- Quote all variables: `"$variable"` not `$variable`
- Use `[[ ]]` for conditionals, not `[ ]`
- Prefer `$()` over backticks for command substitution
- Use meaningful variable names in UPPER_CASE for globals
- Add comments for non-obvious logic
- Use functions for reusable code blocks

### Error Handling

- Check command success before proceeding
- Provide user-friendly error messages via `notify()`
- Clean up resources on failure (see `cleanup_stale()`)
- Use appropriate exit codes (0 for success, 1 for errors)

### State Management

The script uses three core state files:
1. **Lock file** (`$LOCK_FILE`) - Prevents concurrent execution
2. **PID file** (`$PID_FILE`) - Tracks active recording process
3. **Audio file** (`$AUDIO_FILE`) - Temporary recording storage

Always maintain consistency:
- Create PID file when starting recording
- Remove PID file when stopping
- Clean up stale state on startup
- Use file descriptor 200 for locking (`exec 200>"$LOCK_FILE"`)

## Testing

### Manual Testing

```bash
# Test device detection
./bin/ears --list
./bin/ears --select  # Requires fzf

# Test server configuration
./bin/ears --server http://localhost:8178
./bin/ears --server  # Show current

# Test end-to-end
# 1. Start whisper.cpp server
# 2. Start ydotool daemon
# 3. Run ./bin/ears
# 4. Speak
# 5. Run ./bin/ears again
# 6. Verify text appears
```

### Component Testing

```bash
# Test PipeWire recording
pw-record --target YOUR_DEVICE test.wav
# Ctrl+C after a few seconds
paplay test.wav

# Test whisper.cpp API
curl -sf http://localhost:8178/health
curl -X POST http://localhost:8178/inference \
  -F "file=@test.wav" \
  -F "response_format=json" | jq

# Test ydotool
ydotool type "test message"
```

### Debug Logging

Check runtime logs:
```bash
tail -f $XDG_RUNTIME_DIR/ears/debug.log
```

## Commit Guidelines

- Use conventional commits:
  - `feat:` - New features
  - `fix:` - Bug fixes
  - `docs:` - Documentation changes
  - `refactor:` - Code refactoring
  - `test:` - Testing improvements
  - `chore:` - Maintenance tasks

- Keep commits focused and atomic
- Write clear, descriptive messages
- Reference issues when applicable

Examples:
```
feat: add support for custom sound files
fix: prevent race condition in PID file cleanup
docs: add troubleshooting section for ydotool
refactor: extract audio recording to separate function
```

## Pull Request Guidelines

- Create a PR for all changes
- Include clear description of changes
- Test thoroughly before submitting
- Update documentation if behavior changes
- Link to related issues

## Common Tasks

### Adding a New Feature

1. Read the existing code to understand the architecture
2. Implement the feature in `bin/ears`
3. Test manually with all edge cases
4. Update README.md with usage instructions
5. Update help text in the script (`--help`)
6. Consider backward compatibility with existing configs

### Fixing a Bug

1. Reproduce the issue
2. Add debug logging if needed
3. Implement the fix
4. Test the fix thoroughly
5. Ensure no regression in other features
6. Document the fix in commit message

### Improving Documentation

1. Keep README.md as the primary user-facing documentation
2. Keep CLAUDE.md (this file) for development guidance
3. Ensure code examples are tested and work
4. Use clear, concise language
5. Include troubleshooting steps for common issues

## Architecture Decisions

### Why Bash?

- Single file deployment (no dependencies beyond shell)
- Direct access to system tools (PipeWire, ydotool, etc.)
- Lightweight and fast
- Easy to debug and modify
- No compilation required

### Why PipeWire?

- Modern Linux audio stack (replacing PulseAudio)
- Better device isolation (avoids OBS conflicts)
- Native support in recent distros
- Explicit device targeting (`--target`)

### Why whisper.cpp Server?

- Fast C++ implementation of Whisper
- GPU acceleration support
- Simple REST API
- Runs as a separate service (can be remote)
- Better resource management than loading models per-request

### Why ydotool?

- Works on Wayland and X11
- Types text without clipboard interference
- Simulates real keyboard input
- Works across all applications

### State Management Design

Uses filesystem-based state for simplicity:
- Lock files prevent race conditions
- PID files track process lifecycle
- `$XDG_RUNTIME_DIR` ensures automatic cleanup on logout
- No database or complex state management needed

## Security Considerations

When working on this project:
- Never log audio content or transcriptions
- Default to localhost for whisper server
- Validate all user input (URLs, file paths)
- Clean up temporary files immediately
- Don't introduce remote code execution vectors
- Be mindful of audio recording privacy

## Future Enhancement Ideas

Potential areas for improvement (not a roadmap):
- Support for multiple whisper.cpp servers (load balancing)
- Custom post-processing filters for transcriptions
- Integration with clipboard instead of typing
- Audio preprocessing (noise reduction, normalization)
- Language detection and model switching
- Wake word detection for hands-free operation
- Web interface for configuration
- Systemd service integration
- Package creation (deb, rpm, AUR)

When implementing new features, maintain the core philosophy:
- Keep it simple and focused
- Maintain single-file simplicity where possible
- Don't break existing functionality
- Document all new features thoroughly
