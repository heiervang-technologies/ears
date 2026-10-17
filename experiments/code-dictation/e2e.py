"""Audio -> Qwen3-ASR (scope as context) -> spoken.py -> code."""
import sys, time
from probe import tts, asr
from spoken import python_line, bash_line

SCOPE = ["parse_args", "argv", "RequestHandler", "MAX_RETRIES", "items", "self", "numpy", "np"]
for i, line in enumerate(l.strip() for l in open(sys.argv[2]) if l.strip()):
    p = f"/tmp/e2e-{i}.wav"
    tts(line, p)
    heard, dt = asr(p, system=" ".join(SCOPE) if sys.argv[1] == "py" else None)
    t = time.time()
    code = (python_line if sys.argv[1] == "py" else bash_line)(heard, SCOPE)
    print(f"{dt*1000:4.0f}+{(time.time()-t)*1000:.0f}ms | said {line!r}\n         heard {heard!r}\n         code  {code!r}")
