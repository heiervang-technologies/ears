# ASR/STT API Signatures Research & Configurable System Design

> Research document for extending `ears` to support arbitrary ASR API backends.
> Generated: 2026-02-18

## Table of Contents

1. [Current Implementation (OpenAI-Compatible)](#1-current-implementation-openai-compatible)
2. [Provider API Signatures](#2-provider-api-signatures)
3. [Comparison Table](#3-comparison-table)
4. [Similarity Analysis](#4-similarity-analysis)
5. [Configurable JSON Schema Design](#5-configurable-json-schema-design)
6. [Configuration Examples](#6-configuration-examples)
7. [Security Considerations](#7-security-considerations)
8. [Recommendations](#8-recommendations)

---

## 1. Current Implementation (OpenAI-Compatible)

`ears` currently uses the OpenAI-compatible `/v1/audio/transcriptions` endpoint (see `src/whisper.rs`).

### What We Do Today

| Aspect | Value |
|--------|-------|
| **Endpoint** | `POST {server_url}/v1/audio/transcriptions` |
| **Content-Type** | `multipart/form-data` |
| **Auth** | None (local whisper.cpp server) |
| **Audio field** | `file` — raw bytes with filename `recording.wav`, MIME `audio/wav` |
| **Parameters** | `response_format=json`, optional `language` |
| **Response** | `{"text": "..."}` |
| **Health check** | `GET {server_url}/health` |

The client sends a multipart form with the WAV file as `file`, requests JSON response format, and extracts `text` from the response. Language is optional (omitted = auto-detect). Retry logic uses exponential backoff.

### Compatible Servers (Drop-in)

These servers implement the same endpoint and work with the current code unchanged:

- **whisper.cpp** (`whisper-server`) — the primary target
- **faster-whisper-server** — Python wrapper, same API
- **Groq** — cloud service at `https://api.groq.com/openai/v1/audio/transcriptions` (needs `Authorization: Bearer` + `model` field)
- **OpenAI** — the canonical implementation at `https://api.openai.com/v1/audio/transcriptions` (needs `Authorization: Bearer` + `model` field)

---

## 2. Provider API Signatures

### 2.1 OpenAI Whisper API

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://api.openai.com/v1/audio/transcriptions` |
| **Method** | POST |
| **Request format** | `multipart/form-data` |
| **Auth** | `Authorization: Bearer <API_KEY>` |
| **Audio field** | `file` — audio file (flac, mp3, mp4, mpeg, mpga, m4a, ogg, wav, webm) |
| **Required fields** | `file`, `model` (e.g. `whisper-1`, `gpt-4o-transcribe`) |
| **Optional fields** | `language`, `prompt`, `response_format` (json/text/srt/verbose_json/vtt), `temperature`, `timestamp_granularities` |
| **Response (json)** | `{"text": "transcribed text here"}` |
| **Response (verbose_json)** | `{"task":"transcribe", "language":"english", "duration":8.47, "text":"...", "words":[...]}` |
| **Streaming** | Not supported for `whisper-1`; supported for `gpt-4o-transcribe` models |
| **File size limit** | 25 MB |
| **Pricing** | $0.006/min |

**Key difference from ears' current impl:** Requires `Authorization: Bearer` header and `model` field.

### 2.2 Groq (Whisper-Compatible)

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://api.groq.com/openai/v1/audio/transcriptions` |
| **Method** | POST |
| **Request format** | `multipart/form-data` |
| **Auth** | `Authorization: Bearer <GROQ_API_KEY>` |
| **Audio field** | `file` — same formats as OpenAI |
| **Required fields** | `file`, `model` (e.g. `whisper-large-v3`, `whisper-large-v3-turbo`) |
| **Optional fields** | `language`, `prompt`, `response_format` (json/text/verbose_json — NO srt/vtt), `temperature` |
| **Response (json)** | `{"text": "transcribed text here"}` |
| **Streaming** | Not supported |
| **File size limit** | 25 MB (URL for larger) |
| **Pricing** | ~$0.00067/min |

**Differences from OpenAI:** Fewer output formats (no srt/vtt). Same endpoint path under `/openai/` prefix. Downsamples to 16kHz mono internally. ~9x cheaper than OpenAI. 216x real-time speed on Groq's LPU.

### 2.3 Deepgram

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://api.deepgram.com/v1/listen` |
| **Method** | POST |
| **Request format** | **Three options:** (1) JSON body with `{"url": "..."}` + `Content-Type: application/json`, (2) Raw binary audio body + `Content-Type: audio/wav` (or appropriate MIME), (3) Callback mode (async) |
| **Auth** | `Authorization: Token <API_KEY>` (note: `Token`, not `Bearer`) |
| **Audio delivery** | Either `url` in JSON body, or raw binary in request body |
| **Parameters** | Query string: `model`, `language`, `smart_format`, `punctuate`, `diarize`, `callback`, etc. |
| **Response** | `{"results":{"channels":[{"alternatives":[{"transcript":"...","confidence":0.9,"words":[...]}]}]}}` |
| **Text path** | `results.channels[0].alternatives[0].transcript` |
| **Streaming** | WebSocket at `wss://api.deepgram.com/v1/listen` |
| **File size limit** | 2 GB |
| **Pricing** | $0.0043/min (Nova-2) |

**Key differences:** Completely different request format (raw binary body, not multipart). Different auth header prefix (`Token` vs `Bearer`). Parameters via query string, not form fields. Deeply nested response structure. Very flexible input — URL, raw binary, or streaming WebSocket.

### 2.4 AssemblyAI

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://api.assemblyai.com/v2/transcript` |
| **Method** | POST |
| **Request format** | `application/json` — JSON body with `audio_url` |
| **Auth** | `Authorization: <API_KEY>` (bare key, no prefix) |
| **Audio delivery** | Must provide `audio_url` — a publicly accessible URL to the audio file |
| **Required fields** | `audio_url` |
| **Optional fields** | `speech_model`, `language_code`, `speaker_labels`, `webhook_url`, `punctuate`, `redact_pii`, etc. |
| **Submit response** | `{"id": "abc123", "status": "queued", "text": null}` |
| **Poll endpoint** | `GET https://api.assemblyai.com/v2/transcript/{id}` |
| **Completed response** | `{"id":"abc123", "status":"completed", "text":"transcribed text", "words":[...], "confidence":0.95}` |
| **Text path** | `text` (top-level) |
| **Streaming** | WebSocket API (real-time, ~300ms P50 latency) |
| **Pricing** | $0.15/hr (Universal), $0.27/hr (Slam-1) |

**Key differences:** Fundamentally async — submit URL, then poll. No direct file upload in the primary API (audio must be at a URL). Different auth header format (bare key, no Bearer/Token prefix). Two-step workflow (submit + poll). EU endpoint available.

### 2.5 Google Cloud Speech-to-Text v2

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://speech.googleapis.com/v2/projects/{PROJECT}/locations/{LOCATION}/recognizers/{RECOGNIZER}:recognize` |
| **Method** | POST |
| **Request format** | `application/json` — JSON body with base64-encoded audio |
| **Auth** | `Authorization: Bearer <OAUTH2_TOKEN>` (OAuth2 access token from `gcloud auth print-access-token` or service account) |
| **Audio delivery** | `content` — base64-encoded audio in JSON body, OR `uri` — GCS URI (`gs://bucket/file`) |
| **Required fields** | `config` (with `language_codes`, `model`, `auto_decoding_config`), audio (`content` or `uri`) |
| **Response** | `{"results":[{"alternatives":[{"transcript":"...","confidence":0.98}],"resultEndTime":"1.770s"}],"totalBilledTime":"15s"}` |
| **Text path** | `results[0].alternatives[0].transcript` |
| **Sync limit** | 1 minute of audio |
| **Async** | Long-running operation for audio up to 480 minutes |
| **Streaming** | gRPC streaming (not REST) |
| **Pricing** | $0.016/15s (standard), varies by model |

**Key differences:** Complex endpoint URL with project/location/recognizer hierarchy. OAuth2 token auth (not simple API key). Base64-encoded audio in JSON (not multipart or raw binary). GCS URI support. Named recognizer configurations. Complex nested response. gRPC-first design (REST is secondary).

### 2.6 AWS Transcribe

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://transcribe.{region}.amazonaws.com` |
| **Method** | POST |
| **Request format** | `application/x-amz-json-1.1` — JSON body |
| **Auth** | AWS Signature V4 (`Authorization: AWS4-HMAC-SHA256 ...`) |
| **Headers** | `x-amz-target: com.amazonaws.transcribe.Transcribe.StartTranscriptionJob`, `x-amz-date`, `x-amz-content-sha256` |
| **Audio delivery** | `Media.MediaFileUri` — S3 URI only (`s3://bucket/file`) |
| **Required fields** | `TranscriptionJobName`, `Media.MediaFileUri`, one of `LanguageCode`/`IdentifyLanguage`/`IdentifyMultipleLanguages` |
| **Optional fields** | `MediaFormat`, `MediaSampleRateHertz`, `OutputBucketName`, `OutputKey`, `Settings` (vocabulary, channel ID, etc.) |
| **Submit response** | `{"TranscriptionJob":{"TranscriptionJobName":"...","TranscriptionJobStatus":"IN_PROGRESS",...}}` |
| **Result retrieval** | Poll `GetTranscriptionJob`, then download from `TranscriptFileUri` (S3 URL) |
| **Streaming** | WebSocket with HTTP/2 upgrade |
| **Pricing** | $0.024/min (standard), $0.006/min (batch) |

**Key differences:** Completely async batch-only for REST API. Audio must be in S3 — no direct upload. AWS SigV4 auth is the most complex auth scheme. Job-based workflow (submit → poll → download). Results written to S3. Cannot send raw audio over HTTP in a simple POST.

### 2.7 Azure Speech Services

Azure has two relevant REST APIs:

#### 2.7a Short Audio REST API (Legacy, Synchronous)

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://{region}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1` |
| **Method** | POST |
| **Request format** | Raw audio binary body |
| **Content-Type** | `audio/wav; codecs=audio/pcm; samplerate=16000` (or other supported formats) |
| **Auth** | `Ocp-Apim-Subscription-Key: <KEY>` OR `Authorization: Bearer <TOKEN>` (token from `issueToken` endpoint, valid 10 min) |
| **Parameters** | Query string: `language`, `format` (simple/detailed) |
| **Response (simple)** | `{"RecognitionStatus":"Success","DisplayText":"...","Offset":...,"Duration":...}` |
| **Text path** | `DisplayText` |
| **Limit** | ~60 seconds of audio |

#### 2.7b Fast Transcription API (Modern, Synchronous)

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://{region}.api.cognitive.microsoft.com/speechtotext/transcriptions:transcribe?api-version=2025-10-15` |
| **Method** | POST |
| **Request format** | `multipart/form-data` with audio file and JSON config |
| **Auth** | `Ocp-Apim-Subscription-Key: <KEY>` |
| **Audio field** | Audio file as form part |
| **Config** | JSON definition part with `locales`, `profanityFilterMode`, etc. |
| **Pricing** | $0.66/hr |

#### 2.7c Batch Transcription API (Async)

| Aspect | Details |
|--------|---------|
| **Endpoint** | `POST https://{region}.api.cognitive.microsoft.com/speechtotext/v3.2/transcriptions` |
| **Request format** | `application/json` |
| **Auth** | `Ocp-Apim-Subscription-Key: <KEY>` |
| **Audio delivery** | `contentUrls` array or `contentContainerUrl` (Azure Blob Storage) |
| **Required fields** | `locale`, `displayName`, `contentUrls`/`contentContainerUrl` |

**Key differences:** Multiple API surfaces (legacy short audio, fast transcription, batch). Region-embedded endpoint URLs. Unique auth header name (`Ocp-Apim-Subscription-Key`). Token exchange flow for Bearer auth. Short audio API sends raw binary body (not multipart). Complex ecosystem.

### 2.8 Rev.ai

| Aspect | Details |
|--------|---------|
| **Base URL** | `https://api.rev.ai/speechtotext/v1` |
| **Submit endpoint** | `POST /jobs` |
| **Method** | POST |
| **Request format** | `application/json` with `source_config.url`, OR `multipart/form-data` for direct upload |
| **Auth** | `Authorization: Bearer <ACCESS_TOKEN>` |
| **Audio delivery** | URL via `media_url`/`source_config.url` in JSON, or file upload via multipart |
| **Optional fields** | `language`, `metadata`, `callback_url`, `skip_diarization`, `skip_punctuation`, etc. |
| **Submit response** | `{"id":"job_id","status":"in_progress","type":"async",...}` |
| **Poll endpoint** | `GET /jobs/{id}` |
| **Transcript endpoint** | `GET /jobs/{id}/transcript` with `Accept: application/vnd.rev.transcript.v1.0+json` |
| **Completed response** | `{"monologues":[{"speaker":0,"elements":[{"type":"text","value":"hello","ts":0.5,"end_ts":0.8,"confidence":0.99},...]}]}` |
| **Text path** | `monologues[*].elements[*].value` (must be concatenated) |
| **Streaming** | WebSocket at `wss://api.rev.ai/speechtotext/v1/stream` |
| **File size limit** | 2 GB (direct upload) |
| **Pricing** | $0.02/min (async), varies for streaming |

**Key differences:** Async job-based workflow (submit → poll → fetch transcript). Transcript is a separate GET endpoint with custom Accept header. Complex response structure (monologues with elements). Supports both URL and multipart upload. Bearer auth.

---

## 3. Comparison Table

| Provider | Endpoint Style | Request Format | Audio Delivery | Auth Method | Auth Header | Response Text Path | Sync? | Streaming? |
|----------|---------------|----------------|----------------|-------------|-------------|-------------------|-------|------------|
| **OpenAI** | `/v1/audio/transcriptions` | multipart/form-data | `file` field | API Key | `Authorization: Bearer <key>` | `text` | ✅ | Limited |
| **Groq** | `/openai/v1/audio/transcriptions` | multipart/form-data | `file` field | API Key | `Authorization: Bearer <key>` | `text` | ✅ | ❌ |
| **whisper.cpp** | `/v1/audio/transcriptions` | multipart/form-data | `file` field | None | None | `text` | ✅ | ❌ |
| **Deepgram** | `/v1/listen` | raw binary OR JSON | Body bytes or `url` in JSON | API Key | `Authorization: Token <key>` | `results.channels[0].alternatives[0].transcript` | ✅ | WebSocket |
| **AssemblyAI** | `/v2/transcript` | JSON | `audio_url` in JSON | API Key | `Authorization: <key>` | `text` (after polling) | ❌ (async) | WebSocket |
| **Google STT v2** | `/v2/projects/.../recognizers/_:recognize` | JSON | base64 `content` or GCS `uri` | OAuth2 | `Authorization: Bearer <token>` | `results[0].alternatives[0].transcript` | ✅ (≤1min) | gRPC only |
| **AWS Transcribe** | Regional endpoint | JSON (amz-json-1.1) | S3 URI only | SigV4 | `Authorization: AWS4-HMAC-SHA256 ...` | Download from S3 | ❌ (async) | WebSocket |
| **Azure (short)** | `/{region}.stt.speech.microsoft.com/...` | raw binary body | Body bytes | Key or Token | `Ocp-Apim-Subscription-Key: <key>` | `DisplayText` | ✅ (≤60s) | ❌ |
| **Azure (fast)** | `/{region}.api.cognitive.microsoft.com/...` | multipart/form-data | File part | Key | `Ocp-Apim-Subscription-Key: <key>` | TBD (newer API) | ✅ | ❌ |
| **Rev.ai** | `/speechtotext/v1/jobs` | JSON or multipart | `media_url` or file upload | Access Token | `Authorization: Bearer <token>` | `monologues[*].elements[*].value` (concat) | ❌ (async) | WebSocket |

---

## 4. Similarity Analysis

### Tier 1: Drop-in Compatible (Minimal Changes Needed)

These use the **exact same** `/v1/audio/transcriptions` endpoint pattern, multipart form data, and `{"text":"..."}` response:

| Provider | Changes Needed |
|----------|----------------|
| **whisper.cpp** | None (current target) |
| **faster-whisper** | None |
| **Groq** | Add `Authorization: Bearer` header + `model` field |
| **OpenAI** | Add `Authorization: Bearer` header + `model` field |

**Implementation:** Just add optional `api_key` and `model` fields to current WhisperClient config. Already being done in task #2.

### Tier 2: Moderate Adaptation (Same Paradigm, Different Details)

These accept direct audio upload and return transcription synchronously, but with different request/response shapes:

| Provider | Key Differences |
|----------|-----------------|
| **Deepgram** | Raw binary body (not multipart), `Token` auth prefix, query-string params, nested response JSON |
| **Azure (short audio)** | Raw binary body, `Ocp-Apim-Subscription-Key` header, different response shape (`DisplayText`) |
| **Azure (fast transcription)** | Multipart but different field layout, `Ocp-Apim-Subscription-Key` header |

**Implementation:** Could be supported with a configurable request/response mapper.

### Tier 3: Fundamentally Different (Async/Job-Based)

These require a completely different workflow — submit audio, then poll for results:

| Provider | Key Differences |
|----------|-----------------|
| **AssemblyAI** | JSON body with `audio_url`, async poll loop, bare API key auth |
| **Rev.ai** | JSON/multipart submit, poll, separate transcript fetch, complex response |
| **Google STT v2** | Base64 audio in JSON, OAuth2 token refresh, complex endpoint URL, GCS URIs |
| **AWS Transcribe** | S3-only audio, SigV4 auth, async job with S3 output |
| **Azure (batch)** | Blob Storage URLs, async job |

**Implementation:** Would need a fundamentally different client pattern (submit → poll → extract). Out of scope for a simple config system — these would need dedicated client implementations.

---

## 5. Configurable JSON Schema Design

### 5.1 Design Goals

1. **Cover Tier 1 and Tier 2 providers** with zero code changes
2. **Keep it simple** — don't try to solve async/job-based APIs via config
3. **Variable substitution** for secrets and dynamic values
4. **Safe defaults** — no arbitrary code execution, no shell interpolation

### 5.2 Schema

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ears ASR Provider Configuration",
  "description": "Configuration for a custom synchronous ASR/STT API endpoint",
  "type": "object",
  "required": ["name", "request", "response"],
  "properties": {

    "name": {
      "type": "string",
      "description": "Human-readable provider name (e.g., 'Deepgram Nova-2')"
    },

    "url": {
      "type": "string",
      "description": "Endpoint URL. Supports ${var} substitution for: SERVER, REGION, PROJECT, MODEL.",
      "examples": [
        "https://api.deepgram.com/v1/listen?model=nova-2",
        "${SERVER}/v1/audio/transcriptions",
        "https://${REGION}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?language=${LANGUAGE}"
      ]
    },

    "method": {
      "type": "string",
      "enum": ["POST"],
      "default": "POST",
      "description": "HTTP method. Currently only POST is supported."
    },

    "headers": {
      "type": "object",
      "description": "HTTP headers. Values support ${var} substitution. ${API_KEY} is resolved from config or env var EARS_API_KEY.",
      "additionalProperties": { "type": "string" },
      "examples": [
        {
          "Authorization": "Bearer ${API_KEY}",
          "Content-Type": "multipart/form-data"
        },
        {
          "Authorization": "Token ${API_KEY}",
          "Content-Type": "audio/wav"
        },
        {
          "Ocp-Apim-Subscription-Key": "${API_KEY}"
        }
      ]
    },

    "request": {
      "type": "object",
      "required": ["format"],
      "properties": {

        "format": {
          "type": "string",
          "enum": ["multipart", "raw_binary", "json_base64", "json_url"],
          "description": "How to send audio data to the API.",
          "x-descriptions": {
            "multipart": "multipart/form-data with audio as a named file part (OpenAI-style)",
            "raw_binary": "Raw audio bytes in the request body (Deepgram-style)",
            "json_base64": "JSON body with base64-encoded audio (Google-style)",
            "json_url": "JSON body with a URL pointing to the audio file (AssemblyAI-style, requires hosting)"
          }
        },

        "multipart_config": {
          "type": "object",
          "description": "Config for format=multipart",
          "properties": {
            "file_field": {
              "type": "string",
              "default": "file",
              "description": "Form field name for the audio file"
            },
            "filename": {
              "type": "string",
              "default": "recording.wav",
              "description": "Filename to use in the multipart part"
            },
            "mime_type": {
              "type": "string",
              "default": "audio/wav",
              "description": "MIME type for the audio file"
            },
            "extra_fields": {
              "type": "object",
              "description": "Additional form text fields (e.g., model, response_format, language). Values support ${var} substitution.",
              "additionalProperties": { "type": "string" }
            }
          }
        },

        "raw_binary_config": {
          "type": "object",
          "description": "Config for format=raw_binary. Audio bytes sent as request body.",
          "properties": {
            "content_type": {
              "type": "string",
              "default": "audio/wav",
              "description": "Content-Type header for the audio body"
            },
            "query_params": {
              "type": "object",
              "description": "Query parameters appended to URL. Values support ${var} substitution.",
              "additionalProperties": { "type": "string" }
            }
          }
        },

        "json_base64_config": {
          "type": "object",
          "description": "Config for format=json_base64. JSON body with base64 audio.",
          "properties": {
            "body_template": {
              "type": "object",
              "description": "JSON body template. ${AUDIO_BASE64} is replaced with base64-encoded audio. Other ${var} substitutions supported."
            }
          }
        }
      }
    },

    "response": {
      "type": "object",
      "required": ["text_path"],
      "properties": {
        "text_path": {
          "type": "string",
          "description": "JSONPath-like dot notation to extract transcription text from the response. Array indices use [N] syntax.",
          "examples": [
            "text",
            "results.channels[0].alternatives[0].transcript",
            "results[0].alternatives[0].transcript",
            "DisplayText"
          ]
        },
        "encoding": {
          "type": "string",
          "enum": ["utf-8"],
          "default": "utf-8"
        }
      }
    },

    "health_check": {
      "type": "object",
      "description": "Optional health check endpoint configuration",
      "properties": {
        "url": {
          "type": "string",
          "description": "Health check URL. Supports ${var} substitution. If omitted, health check is skipped.",
          "examples": ["${SERVER}/health"]
        },
        "method": {
          "type": "string",
          "enum": ["GET", "HEAD"],
          "default": "GET"
        }
      }
    },

    "timeout_secs": {
      "type": "integer",
      "default": 30,
      "description": "Request timeout in seconds"
    },

    "max_file_size_mb": {
      "type": "integer",
      "description": "Maximum audio file size in MB (informational, for validation)"
    }
  }
}
```

### 5.3 Variable Substitution

Variables use `${VAR_NAME}` syntax and are resolved in this priority order:

| Variable | Source | Description |
|----------|--------|-------------|
| `${SERVER}` | `--server` flag or `~/.config/ears/server` | Base server URL |
| `${API_KEY}` | `--api-key` flag, `~/.config/ears/api_key`, or `EARS_API_KEY` env var | API authentication key |
| `${LANGUAGE}` | `--language` flag or auto-detected | Language code (e.g., `en`) |
| `${MODEL}` | Provider config or `--model` flag | Model identifier |
| `${REGION}` | Provider config | Cloud region (e.g., `us-east-1`) |
| `${PROJECT}` | Provider config | Project ID (Google Cloud) |
| `${AUDIO_BASE64}` | Runtime | Base64-encoded audio data (auto-generated) |

**Variable resolution rules:**
- Undefined variables cause an error at startup (fail-fast)
- Variables are NOT resolved via shell expansion — no `$()`, backticks, or env var leaking
- Only the above allowlisted variables are permitted

### 5.4 Storage Location

Provider configs are stored as JSON files in:
```
~/.config/ears/providers/
├── deepgram.json
├── assemblyai.json
├── openai.json
├── azure.json
└── custom.json
```

Select a provider at runtime:
```bash
ears --provider deepgram
ears --provider ~/.config/ears/providers/custom.json
```

A built-in `openai` provider is the default and matches current behavior.

---

## 6. Configuration Examples

### 6.1 Deepgram (Raw Binary)

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
  "timeout_secs": 30,
  "max_file_size_mb": 2000
}
```

**Usage:**
```bash
export EARS_API_KEY="your-deepgram-key"
ears --provider deepgram
```

### 6.2 OpenAI Whisper (Multipart with Auth)

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
  "max_file_size_mb": 25
}
```

### 6.3 Azure Short Audio (Raw Binary, Custom Auth Header)

```json
{
  "name": "Azure Speech (Short Audio)",
  "url": "https://${REGION}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1?language=${LANGUAGE}&format=simple",
  "headers": {
    "Ocp-Apim-Subscription-Key": "${API_KEY}",
    "Content-Type": "audio/wav; codecs=audio/pcm; samplerate=16000"
  },
  "request": {
    "format": "raw_binary",
    "raw_binary_config": {
      "content_type": "audio/wav; codecs=audio/pcm; samplerate=16000"
    }
  },
  "response": {
    "text_path": "DisplayText"
  },
  "timeout_secs": 15,
  "max_file_size_mb": 10
}
```

**Usage:**
```bash
EARS_API_KEY="your-azure-key" ears --provider azure --region eastus --language en-US
```

### 6.4 Google Cloud Speech-to-Text v2 (JSON with Base64 Audio)

```json
{
  "name": "Google Cloud STT v2",
  "url": "https://speech.googleapis.com/v2/projects/${PROJECT}/locations/global/recognizers/_:recognize",
  "headers": {
    "Authorization": "Bearer ${API_KEY}",
    "Content-Type": "application/json; charset=utf-8"
  },
  "request": {
    "format": "json_base64",
    "json_base64_config": {
      "body_template": {
        "config": {
          "auto_decoding_config": {},
          "language_codes": ["${LANGUAGE}"],
          "model": "long"
        },
        "content": "${AUDIO_BASE64}"
      }
    }
  },
  "response": {
    "text_path": "results[0].alternatives[0].transcript"
  },
  "timeout_secs": 30
}
```

**Note:** Google's OAuth2 token must be generated externally (e.g., `gcloud auth print-access-token`) and passed as `API_KEY`. Full OAuth2 flow is out of scope for the config system.

### 6.5 Local whisper.cpp Server (Current Default)

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

---

## 7. Security Considerations

### MUST enforce (non-configurable)

| Guardrail | Reason |
|-----------|--------|
| **HTTPS required for remote URLs** | Prevent credentials from being sent over plaintext HTTP. Allow HTTP only for localhost/127.0.0.1/[::1] addresses. |
| **No shell interpolation in variables** | Variables use simple string substitution only. No `$()`, backticks, or env var expansion beyond the allowlist. |
| **API key masking in logs** | Never log the resolved value of `${API_KEY}`. Mask as `***` in debug output. |
| **Variable allowlist** | Only recognized variable names are permitted. Reject configs with unknown `${VAR}` patterns. |
| **No arbitrary file reads** | The `json_url` format type cannot reference `file://` URIs. |
| **Timeout enforcement** | Maximum timeout capped at 120 seconds regardless of config. |
| **Response size limit** | Cap response body at 10 MB to prevent memory exhaustion. |

### SHOULD enforce (recommended)

| Guardrail | Reason |
|-----------|--------|
| **Config file permissions** | Warn if provider config files are world-readable (they may contain API keys inline, though env vars are preferred). |
| **URL validation** | Reject obviously malicious URLs (e.g., internal IP ranges unless explicitly allowed). |
| **Certificate verification** | Always verify TLS certificates. No `--insecure` equivalent in configs. |

### Out of scope for config system

| Feature | Reason |
|---------|--------|
| **AWS SigV4 signing** | Too complex for generic config — needs dedicated implementation |
| **OAuth2 token refresh** | Requires a separate token lifecycle — use external tooling to provide a fresh token |
| **Async job polling** | Fundamentally different workflow — needs dedicated client code, not a config template |
| **WebSocket streaming** | Different protocol entirely — would need its own config system |

---

## 8. Recommendations

### Short-term (Easy Wins)

1. **Add optional `api_key` and `model` fields** to the existing WhisperClient — this unlocks Groq and OpenAI with minimal code changes. (Already task #2.)
2. **Ship built-in provider configs** for whisper.cpp, OpenAI, and Groq as hardcoded defaults.

### Medium-term (Configurable Provider System)

3. **Implement the JSON config system** described above to support Tier 2 providers (Deepgram, Azure short audio) without code changes.
4. **Start with 3 request formats:** `multipart`, `raw_binary`, `json_base64`. These cover the most common synchronous APIs.
5. **Add `--provider` CLI flag** and provider config directory.

### Long-term (If Demand Exists)

6. **Dedicated AssemblyAI/Rev.ai clients** with async job polling — only worth building if users request it, since the async workflow is fundamentally different.
7. **WebSocket streaming support** for Deepgram and AssemblyAI real-time APIs — significant engineering effort but would enable live transcription from these providers.
8. **Google Cloud STT v2** with proper OAuth2 service account support — only if there's demand for it.

### Not Recommended

- **AWS Transcribe support** — The S3-only, SigV4-auth, fully-async workflow is too far from ears' direct-upload-and-transcribe model. Users needing AWS Transcribe should use the AWS CLI/SDK directly.
- **Azure batch transcription** — Same reasoning. The async Blob Storage workflow doesn't fit ears' real-time use case.

---

## References

- [OpenAI API — Create Transcription](https://platform.openai.com/docs/api-reference/audio/createTranscription)
- [Groq Speech-to-Text Docs](https://console.groq.com/docs/speech-to-text)
- [Deepgram Pre-Recorded Audio](https://developers.deepgram.com/docs/getting-started-with-pre-recorded-audio)
- [AssemblyAI Transcripts API](https://www.assemblyai.com/docs/api-reference/transcripts/submit)
- [Google Cloud Speech-to-Text v2](https://cloud.google.com/speech-to-text/v2/docs/reference/rest)
- [AWS Transcribe API Reference](https://docs.aws.amazon.com/transcribe/latest/APIReference/API_StartTranscriptionJob.html)
- [Azure Speech-to-Text REST API](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text)
- [Azure Short Audio REST API](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short)
- [Rev.ai Async API Docs](https://docs.rev.ai/api/asynchronous/)
