# Deploying Qwen3-ASR-1.7B with vLLM

Complete guide to running Qwen3-ASR-1.7B as an OpenAI-compatible speech-to-text API.

## Prerequisites

- NVIDIA GPU with at least 8GB VRAM (tested on RTX 3060 12GB, RTX 3090 24GB)
- NVIDIA driver 535+ (tested with 590.48.01 / CUDA 13.1)
- Python 3.12
- ~10GB disk for the venv, ~4GB for model weights

## 1. Create a Python Virtual Environment

```bash
python3.12 -m venv /tmp/qwen3-asr-venv
```

## 2. Install vLLM Nightly

Qwen3-ASR requires vLLM nightly (0.16.0rc2+). The stable release does not include Qwen3-ASR support.

```bash
/tmp/qwen3-asr-venv/bin/pip install https://wheels.vllm.ai/nightly/vllm-1.0.0.dev-cp38-abi3-manylinux1_x86_64.whl
```

If you hit a `Cannot uninstall distro` error (common on Debian/Ubuntu):

```bash
/tmp/qwen3-asr-venv/bin/pip install --ignore-installed distro \
  https://wheels.vllm.ai/nightly/vllm-1.0.0.dev-cp38-abi3-manylinux1_x86_64.whl
```

## 3. Install Audio Support

The `/v1/audio/transcriptions` endpoint (OpenAI-compatible) requires the audio extra:

```bash
/tmp/qwen3-asr-venv/bin/pip install 'vllm[audio]'
```

This pulls in `soundfile`, `librosa`, and audio processing dependencies.

## 4. Download the Model

```bash
/tmp/qwen3-asr-venv/bin/python -c "from huggingface_hub import snapshot_download; snapshot_download('Qwen/Qwen3-ASR-1.7B')"
```

Or if you have a shared HuggingFace cache, set `HF_HOME` to point at it.

## 5. Start the Server

### Minimal (text API only)

```bash
/tmp/qwen3-asr-venv/bin/python -m vllm.entrypoints.openai.api_server \
  --model Qwen/Qwen3-ASR-1.7B \
  --host 0.0.0.0 \
  --port 8000 \
  --dtype bfloat16 \
  --max-model-len 2048
```

### With GPU memory limit (shared GPU)

```bash
/tmp/qwen3-asr-venv/bin/python -m vllm.entrypoints.openai.api_server \
  --model Qwen/Qwen3-ASR-1.7B \
  --host 0.0.0.0 \
  --port 8000 \
  --dtype bfloat16 \
  --max-model-len 2048 \
  --gpu-memory-utilization 0.30
```

### Full VRAM (dedicated GPU)

```bash
/tmp/qwen3-asr-venv/bin/python -m vllm.entrypoints.openai.api_server \
  --model Qwen/Qwen3-ASR-1.7B \
  --host 0.0.0.0 \
  --port 8000 \
  --dtype auto \
  --max-model-len 4096 \
  --gpu-memory-utilization 0.90
```

### Background daemon

```bash
nohup /tmp/qwen3-asr-venv/bin/python -m vllm.entrypoints.openai.api_server \
  --model Qwen/Qwen3-ASR-1.7B \
  --host 0.0.0.0 \
  --port 8000 \
  --dtype bfloat16 \
  --max-model-len 2048 \
  --gpu-memory-utilization 0.30 \
  > /tmp/qwen3-asr-server.log 2>&1 &
```

## 6. Verify

```bash
# Health check
curl http://localhost:8000/health

# List models
curl http://localhost:8000/v1/models

# Transcribe audio
curl http://localhost:8000/v1/audio/transcriptions \
  -F "file=@recording.wav" \
  -F "model=Qwen/Qwen3-ASR-1.7B"
```

Expected response:

```json
{"text": "your transcription here", "usage": {"type": "duration", "seconds": 5}}
```

## 7. Startup Time

The server takes 30-90 seconds to start depending on the GPU:

1. Model weights load (~3-4 seconds)
2. Encoder cache profiling with dummy audio (~5-10 seconds)
3. Triton JIT compilation on first run (~20-60 seconds, cached after)
4. CUDA graph capture (~10-20 seconds)

Subsequent starts are faster due to cached JIT artifacts in `~/.cache/vllm/`.

## GPU Memory Usage

| GPU | `--gpu-memory-utilization` | VRAM Used | Notes |
|-----|---------------------------|-----------|-------|
| RTX 3090 (24GB) | 0.30 | ~7.7 GiB | Shared with TTS and desktop |
| RTX 3060 (12GB) | 0.90 | ~10.8 GiB | Dedicated |

The model itself is ~3.9 GiB. The rest is KV cache and CUDA graphs.

## Containerized Deployment

### Slim Image (copy venv from host)

This is the fast path — build a working venv on the host, then copy it into a container.

```dockerfile
FROM python:3.12-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libsndfile1 \
    gcc \
    libc6-dev \
    && rm -rf /var/lib/apt/lists/*

COPY venv /opt/venv

# Fix python symlinks — venv was created with host python
RUN rm -f /opt/venv/bin/python /opt/venv/bin/python3 /opt/venv/bin/python3.12 && \
    ln -s /usr/local/bin/python3.12 /opt/venv/bin/python && \
    ln -s python /opt/venv/bin/python3 && \
    ln -s python /opt/venv/bin/python3.12

ENV PATH="/opt/venv/bin:$PATH"
ENV VIRTUAL_ENV=/opt/venv

ENTRYPOINT ["/opt/venv/bin/python", "-m", "vllm.entrypoints.openai.api_server"]
CMD ["--model", "Qwen/Qwen3-ASR-1.7B", \
     "--host", "0.0.0.0", \
     "--port", "8000", \
     "--dtype", "bfloat16", \
     "--max-model-len", "2048", \
     "--gpu-memory-utilization", "0.30"]
```

Build:

```bash
cp -al /tmp/qwen3-asr-venv ./venv   # hardlink to avoid doubling disk
docker build -f Dockerfile.slim -t qwen3-asr:slim .
```

Key gotchas:
- **gcc is required** — Triton's JIT compiler needs a C compiler at runtime
- **Python symlinks break** — venv `bin/python` symlinks to the host interpreter and must be relinked to the container's `/usr/local/bin/python3.12`
- **libsndfile1 is required** — for audio file decoding in the `/v1/audio/transcriptions` endpoint

### Full Image (from vLLM nightly base)

```dockerfile
FROM vllm/vllm-openai:nightly
RUN pip install --no-cache-dir 'vllm[audio]'
ENTRYPOINT ["python", "-m", "vllm.entrypoints.openai.api_server"]
CMD ["--model", "Qwen/Qwen3-ASR-1.7B", ...]
```

Simpler but the base image is ~8GB to pull.

## Kubernetes Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: qwen3-asr
spec:
  replicas: 1
  selector:
    matchLabels:
      app: qwen3-asr
  template:
    metadata:
      labels:
        app: qwen3-asr
    spec:
      runtimeClassName: nvidia
      nodeSelector:
        kubernetes.io/hostname: <gpu-node>
      containers:
        - name: qwen3-asr
          image: qwen3-asr:slim
          imagePullPolicy: Never    # local image, not from registry
          ports:
            - containerPort: 8000
          env:
            - name: NVIDIA_VISIBLE_DEVICES
              value: all
            - name: NVIDIA_DRIVER_CAPABILITIES
              value: compute,utility
          resources:
            requests:
              cpu: "2"
              memory: 4Gi
            limits:
              cpu: "4"
              memory: 8Gi
          readinessProbe:
            httpGet:
              path: /health
              port: 8000
            initialDelaySeconds: 60
            periodSeconds: 10
            timeoutSeconds: 5
            failureThreshold: 6
          livenessProbe:
            httpGet:
              path: /health
              port: 8000
            initialDelaySeconds: 120
            periodSeconds: 30
            timeoutSeconds: 10
            failureThreshold: 3
          volumeMounts:
            - name: huggingface-cache
              mountPath: /root/.cache/huggingface
      volumes:
        - name: huggingface-cache
          hostPath:
            path: /path/to/huggingface/cache
            type: DirectoryOrCreate
---
apiVersion: v1
kind: Service
metadata:
  name: qwen3-asr
spec:
  type: NodePort
  selector:
    app: qwen3-asr
  ports:
    - port: 8000
      targetPort: 8000
      nodePort: 30189
```

Import the image into containerd (RKE2):

```bash
docker save qwen3-asr:slim | sudo ctr \
  --address /run/k3s/containerd/containerd.sock \
  -n k8s.io images import --all-platforms -
```

Mount the HuggingFace cache as a hostPath volume so the model doesn't re-download on every pod restart.

## Vast AI / Cloud GPU Deployment

1. Rent a GPU instance with CUDA 12.x+ and Python 3.12
2. SSH in and follow steps 1-6 above
3. Bind vLLM to a port that's exposed externally, or use an SSH tunnel:

```bash
# From your local machine
ssh -f -N -L 38200:localhost:8000 -p <ssh-port> root@<vast-ip>
```

Then configure your client to use `http://localhost:38200`.

## ears Integration

Create a profile at `~/.config/ears/config.qwen3-asr.toml`:

```toml
# Local cluster
server = "http://<node-ip>:30189"
model = "Qwen/Qwen3-ASR-1.7B"
language = "en"
```

Use with: `ears -p qwen3-asr`

Set as default: `ears profile qwen3-asr`

## Troubleshooting

### "Failed to find C compiler"

```
torch._inductor.exc.InductorError: RuntimeError: Failed to find C compiler.
```

Install gcc: `apt install gcc libc6-dev` (Debian/Ubuntu) or `pacman -S gcc` (Arch).

### "Please install vllm[audio]"

```
{"error":{"message":"Please install vllm[audio] for audio support"}}
```

Run: `pip install 'vllm[audio]'`

### "No module named 'vllm'"

You're using the system python instead of the venv. Use the full path:

```bash
/tmp/qwen3-asr-venv/bin/python -m vllm.entrypoints.openai.api_server ...
```

### Engine core fails silently

Check logs for the `EngineCore_DP0` subprocess output — the root cause is often in a child process that crashes before the parent can report it. Common causes:
- OOM: reduce `--gpu-memory-utilization` or `--max-model-len`
- Missing C compiler (see above)
- CUDA driver mismatch in containers

### Server returns empty on /health

The health endpoint returns 200 with an empty body when healthy. If it returns nothing at all, check firewall rules:

```bash
sudo ufw allow 8000/tcp
```

### Slow first request

The first inference after startup triggers Triton kernel compilation. This is normal and takes 5-30 seconds. Subsequent requests are fast. The compiled kernels are cached in `~/.cache/vllm/`.
