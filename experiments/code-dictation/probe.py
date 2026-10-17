import base64, json, subprocess, sys, urllib.request, time
URL = "http://localhost:30189/v1/chat/completions"
MODEL = "Qwen/Qwen3-ASR-1.7B"

def tts(text, path):
    subprocess.run(["espeak-ng", "-s", "150", "-w", path + ".raw.wav", text], check=True)
    subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", path + ".raw.wav", "-ar", "16000", "-ac", "1", path], check=True)

def asr(path, system=None, grammar=None, max_tokens=96):
    audio = base64.b64encode(open(path, "rb").read()).decode()
    messages = []
    if system:
        messages.append({"role": "system", "content": system})
    messages.append({"role": "user", "content": [
        {"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]})
    body = {"model": MODEL, "messages": messages, "max_tokens": max_tokens, "temperature": 0.0}
    if grammar:
        body["structured_outputs"] = {"grammar": grammar}
    req = urllib.request.Request(URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
    t = time.time()
    out = json.load(urllib.request.urlopen(req, timeout=60))["choices"][0]["message"]["content"]
    return (out.split("<asr_text>", 1)[-1].strip() or "RAW:" + out), time.time() - t

def asr_nbest(path, system=None, n=4, max_tokens=96):
    """Beam-search hypotheses, best first."""
    audio = base64.b64encode(open(path, "rb").read()).decode()
    messages = [{"role": "system", "content": system}] if system else []
    messages.append({"role": "user", "content": [
        {"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]})
    body = {"model": MODEL, "messages": messages, "max_tokens": max_tokens,
            "temperature": 0.0, "n": n, "use_beam_search": True}
    req = urllib.request.Request(URL, json.dumps(body).encode(), {"Content-Type": "application/json"})
    choices = json.load(urllib.request.urlopen(req, timeout=60))["choices"]
    return [c["message"]["content"].split("<asr_text>", 1)[-1].strip() for c in choices]


if __name__ == "__main__":
    cases = [l.rstrip("\n") for l in open(sys.argv[1]) if l.strip()]
    system = open(sys.argv[2]).read() if len(sys.argv) > 2 and sys.argv[2] != "-" else None
    grammar = open(sys.argv[3]).read() if len(sys.argv) > 3 else None
    for i, spoken in enumerate(cases):
        p = f"/tmp/cd-{i}.wav"
        tts(spoken, p)
        text, dt = asr(p, system, grammar)
        print(f"{dt*1000:5.0f}ms | {spoken!r:55} -> {text!r}")
