"""Dry run of the LoRA setup on the meta device: no weights, no GPU, no training.

    PYTHONPATH=<site-packages with peft> python lora_shapes.py <Qwen3-ASR snapshot dir> [rank]

Builds Qwen3-ASR from its config with transformers' native class, puts a LoRA
on the text decoder only (the audio encoder and projector stay frozen), and
prints the trainable parameter count, the adapter size, and the adapter keys
PEFT would save next to the names vLLM's Qwen3-ASR weight mapper turns them
into, so a serving mismatch shows up before any training run.
"""

import re, sys

import torch
from peft import LoraConfig, get_peft_model
from peft.utils import get_peft_model_state_dict
from transformers import AutoConfig, Qwen3ASRForConditionalGeneration

snap = sys.argv[1]
rank = int(sys.argv[2]) if len(sys.argv) > 2 else 16

# Copied from vllm 0.28.0 Qwen3ASRForConditionalGeneration.hf_to_vllm_mapper.
VLLM_PREFIXES = {
    "thinker.lm_head.": "language_model.lm_head.",
    "thinker.model.": "language_model.model.",
    "thinker.": "",
    "model.audio_tower.": "audio_tower.",
    "model.language_model.": "language_model.model.",
    "model.multi_modal_projector.linear_1.": "audio_tower.proj1.",
    "model.multi_modal_projector.linear_2.": "audio_tower.proj2.",
}


def to_vllm(name):
    for old, new in VLLM_PREFIXES.items():
        if name.startswith(old):
            return new + name[len(old):]
    return name


config = AutoConfig.from_pretrained(snap)
with torch.device("meta"):
    model = Qwen3ASRForConditionalGeneration(config)

linear = sorted({n for n, m in model.named_modules() if isinstance(m, torch.nn.Linear)})
print(f"{len(linear)} Linear modules; first decoder ones:")
for n in [n for n in linear if ".layers.0." in n and "audio" not in n]:
    print("  ", n)

# Decoder attention + MLP only: the regex keeps the audio tower out.
target = r".*language_model.*\.(q_proj|k_proj|v_proj|o_proj|gate_proj|up_proj|down_proj)"
cfg = LoraConfig(r=rank, lora_alpha=2 * rank, lora_dropout=0.05, target_modules=target, bias="none")
model = get_peft_model(model, cfg)
trainable = sum(p.numel() for p in model.parameters() if p.requires_grad)
total = sum(p.numel() for p in model.parameters())
print(f"rank {rank}: trainable {trainable / 1e6:.1f} M of {total / 1e9:.2f} B "
      f"({100 * trainable / total:.2f} %), adapter ~{2 * trainable / 2**20:.0f} MiB in bf16")

keys = list(get_peft_model_state_dict(model))
print(f"{len(keys)} adapter tensors; examples (saved -> vLLM):")
for k in keys[:2] + [k for k in keys if "mlp.down_proj" in k][:1]:
    bare = re.sub(r"^base_model\.model\.", "", k)
    print("  ", k, "->", to_vllm(bare))
assert all("audio" not in k for k in keys), "LoRA leaked into the audio tower"
