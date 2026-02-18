# Provider Configuration Guide

`ears` supports configurable ASR (Automatic Speech Recognition) providers through JSON configuration files. This allows you to use different speech-to-text services beyond the default OpenAI-compatible endpoint.

## Table of Contents

1. [Quick Start](#quick-start)
2. [Configuration Location](#configuration-location)
3. [Configuration Schema](#configuration-schema)
4. [Variable Substitution](#variable-substitution)
5. [Provider Examples](#provider-examples)
6. [Security Considerations](#security-considerations)
7. [Troubleshooting](#troubleshooting)

## Quick Start

To use a custom provider:

1. Create a provider configuration file at `~/.config/ears/provider.json`
2. Add your API key to `~/.config/ears/api_key` or set `EARS_API_KEY` environment variable
3. Run `ears` - it will automatically load and use your provider configuration

When no `provider.json` file exists, `ears` uses the default OpenAI-compatible behavior (current behavior with local whisper.cpp servers).

## Configuration Location

Provider configurations are loaded from:
```
~/.config/ears/provider.json
```

Related configuration files:
- `~/.config/ears/api_key` - API key (optional, can use `EARS_API_KEY` env var instead)
- `~/.config/ears/server` - Server URL (used for `${SERVER}` variable)
- `~/.config/ears/language` - Language code (used for `${LANGUAGE}` variable)

## Configuration Schema

A provider configuration JSON file has the following structure:

```json
{
  "name": "Provider Name",
  "url": "https://api.example.com/endpoint",
  "headers": {
    "Authorization": "Bearer ${API_KEY}"
  },
  "request": {
    "format": "multipart",
    "multipart_config": {
      "file_field": "file",
      "filename": "recording.wav",
      "mime_type": "audio/wav",
      "extra_fields": {
        "model": "whisper-1",
        "response_format": "json"
      }
    }
  },
  "response": {
    "text_path": "text"
  },
  "health_check": {
    "url": "${SERVER}/health",
    "method": "GET"
  },
  "timeout_secs": 30
}
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Human-readable provider name (for logs) |
| `url` | Yes | API endpoint URL (supports variable substitution) |
| `headers` | No | HTTP headers (values support variable substitution) |
| `request` | Yes | Request configuration |
| `request.format` | Yes | `"multipart"` or `"raw_binary"` |
| `request.multipart_config` | If `format=multipart` | Multipart form configuration |
| `request.raw_binary_config` | If `format=raw_binary"` | Raw binary request configuration |
| `response` | Yes | Response parsing configuration |
| `response.text_path` | Yes | JSONPath to extract text (e.g., `"text"`, `"results.channels[0].alternatives[0].transcript"`) |
| `health_check` | No | Optional health check endpoint configuration |
| `timeout_secs` | No | Request timeout (default: 30, max: 120) |

## Variable Substitution

Template strings support `${VAR}` variable substitution:

| Variable | Source | Description |
|----------|--------|-------------|
| `${SERVER}` | `~/.config/ears/server` or `EARS_SERVER` env var | Base server URL |
| `${API_KEY}` | `~/.config/ears/api_key` or `EARS_API_KEY` env var | API authentication key |
| `${LANGUAGE}` | `~/.config/ears/language`, `EARS_LANGUAGE`, or auto-detected | Language code (e.g., `en`, `no`) |

**Security notes:**
- Variables use simple string substitution only - no shell interpolation or command execution
- `${API_KEY}` values are automatically masked in debug logs
- Only recognized variable names are permitted (config validation fails on unknown variables)

## Provider Examples

### Example 1: Deepgram (Raw Binary)

Deepgram uses raw audio bytes in the request body with query parameters.

**`~/.config/ears/provider.json`:**
```json
{
  "name": "Deepgram Nova-2",
  "url": "https://api.deepgram.com/v1/listen?model=nova-2&smart_format=true&language=${LANGUAGE}",
  "headers": {
    "Authorization": "Token ${API_KEY}"
  },
  "request": {
    "format": "raw_binary",
    "raw_binary_config": {
      "content_type": "audio/wav"
    }
  },
  "response": {
    "text_path": "results.channels[0].alternatives[0].transcript"
  },
  "timeout_secs": 30
}
```

**Setup:**
```bash
# Set API key
echo "your-deepgram-api-key" > ~/.config/ears/api_key
chmod 600 ~/.config/ears/api_key

# Or use environment variable
export EARS_API_KEY="your-deepgram-api-key"

# Run ears
ears
```

### Example 2: OpenAI Whisper (Multipart with Auth)

OpenAI's Whisper API uses multipart form-data with Bearer authentication.

**`~/.config/ears/provider.json`:**
```json
{
  "name": "OpenAI Whisper",
  "url": "https://api.openai.com/v1/audio/transcriptions",
  "headers": {
    "Authorization": "Bearer ${API_KEY}"
  },
  "request": {
    "format": "multipart",
    "multipart_config": {
      "file_field": "file",
      "filename": "recording.wav",
      "mime_type": "audio/wav",
      "extra_fields": {
        "model": "whisper-1",
        "response_format": "json",
        "language": "${LANGUAGE}"
      }
    }
  },
  "response": {
    "text_path": "text"
  },
  "timeout_secs": 30
}
```

**Setup:**
```bash
echo "sk-your-openai-api-key" > ~/.config/ears/api_key
chmod 600 ~/.config/ears/api_key
ears
```

### Example 3: Groq (Whisper-Compatible, High Speed)

Groq provides Whisper-compatible endpoints with ~9x lower cost and 216x real-time speed.

**`~/.config/ears/provider.json`:**
```json
{
  "name": "Groq Whisper Large v3 Turbo",
  "url": "https://api.groq.com/openai/v1/audio/transcriptions",
  "headers": {
    "Authorization": "Bearer ${API_KEY}"
  },
  "request": {
    "format": "multipart",
    "multipart_config": {
      "file_field": "file",
      "filename": "recording.wav",
      "mime_type": "audio/wav",
      "extra_fields": {
        "model": "whisper-large-v3-turbo",
        "response_format": "json",
        "language": "${LANGUAGE}"
      }
    }
  },
  "response": {
    "text_path": "text"
  },
  "timeout_secs": 30
}
```

**Setup:**
```bash
# Get API key from https://console.groq.com/
echo "gsk_your-groq-api-key" > ~/.config/ears/api_key
chmod 600 ~/.config/ears/api_key
ears
```

### Example 4: Local whisper.cpp Server (Default)

This is the default behavior when no `provider.json` exists. Included here for reference.

**`~/.config/ears/provider.json`:**
```json
{
  "name": "whisper.cpp (local)",
  "url": "${SERVER}/v1/audio/transcriptions",
  "headers": {},
  "request": {
    "format": "multipart",
    "multipart_config": {
      "file_field": "file",
      "filename": "recording.wav",
      "mime_type": "audio/wav",
      "extra_fields": {
        "response_format": "json",
        "language": "${LANGUAGE}"
      }
    }
  },
  "response": {
    "text_path": "text"
  },
  "health_check": {
    "url": "${SERVER}/health",
    "method": "GET"
  },
  "timeout_secs": 30
}
```

**Setup:**
```bash
# Set server URL (default: http://127.0.0.1:8178)
echo "http://localhost:8178" > ~/.config/ears/server

# Run local whisper.cpp server
whisper-server -m /path/to/model.bin

# Run ears (no API key needed)
ears
```

## Security Considerations

### HTTPS Enforcement

- **HTTPS required** for all remote URLs (non-localhost)
- HTTP is **only allowed** for `localhost`, `127.0.0.1`, and `[::1]`
- This prevents API keys from being sent over plaintext connections

### API Key Protection

- **Never log API keys** - values are automatically masked in debug output
- **File permissions** - set `chmod 600 ~/.config/ears/api_key` to prevent unauthorized access
- **Environment variables** - prefer `EARS_API_KEY` env var for temporary use or CI/CD
- **No command injection** - variable substitution is string-based only, no shell expansion

### Configuration Validation

The following checks are performed on startup:

- URL scheme must be `http` or `https`
- Required config sections must be present for the selected format
- Timeout must be between 1-120 seconds
- Unknown `${VARIABLES}` cause startup failure (fail-fast)
- Response text path must not be empty

### Not Supported (Security by Design)

The configuration system does **not** support:

- ❌ AWS SigV4 signing (too complex for JSON config)
- ❌ OAuth2 token refresh (use external tooling to provide fresh tokens)
- ❌ Async job polling (fundamentally different workflow)
- ❌ WebSocket streaming (different protocol)
- ❌ Arbitrary code execution or shell commands
- ❌ File:// URIs for audio (local files only)

## Troubleshooting

### Configuration not loading

Check the log file:
```bash
tail -f ~/.local/state/ears/debug.log
# Or (if XDG_RUNTIME_DIR is set):
tail -f $XDG_RUNTIME_DIR/ears/debug.log
```

Look for messages like:
- `Loaded provider config: <name>` - Success
- `Failed to load provider config: <error>` - Configuration error

### Invalid configuration errors

Common issues:

1. **Missing required fields**
   ```
   Failed to parse provider config: missing field 'name'
   ```
   Solution: Ensure all required fields are present in your JSON.

2. **Undefined variable**
   ```
   Provider configuration error: Undefined variable: API_KEY
   ```
   Solution: Set the variable in `~/.config/ears/api_key` or `EARS_API_KEY` env var.

3. **Invalid URL scheme**
   ```
   Provider configuration error: HTTPS required for remote URLs
   ```
   Solution: Use `https://` for remote services, or `http://localhost` for local servers.

4. **Text path not found**
   ```
   Provider configuration error: Field 'transcript' not found in response
   ```
   Solution: Verify the `response.text_path` matches your provider's actual response structure.

### Testing your configuration

1. **Validate JSON syntax**:
   ```bash
   jq . ~/.config/ears/provider.json
   ```

2. **Test with a short recording**:
   - Start `ears`
   - Press your keybind to start recording
   - Say a short phrase
   - Press keybind again to stop and transcribe
   - Check logs for errors

3. **Enable debug logging**:
   ```bash
   RUST_LOG=debug ears
   ```

### Provider-specific issues

**Deepgram:**
- Ensure you're using `Token` not `Bearer` in the Authorization header
- Check that your API key has credits remaining

**OpenAI:**
- Verify your API key starts with `sk-`
- Check file size limits (25 MB max)
- Ensure the model name is correct (`whisper-1` or `gpt-4o-transcribe`)

**Groq:**
- Use model `whisper-large-v3-turbo` or `whisper-large-v3`
- Note: Groq doesn't support `srt`/`vtt` response formats (use `json` or `text`)

## Further Reading

- [Architecture Documentation](./ARCHITECTURE.md) - How ears works internally
- [Main README](../README.md) - Installation and usage
- [ASR API Research](../ASR_API_SIGNATURES.md) - Comparison of different provider APIs
