#!/usr/bin/env python3
"""
Sync voice samples from PostgreSQL to Hugging Face dataset.

Efficiently adds new samples without re-uploading the entire dataset.
Tracks last synced ID to only process new entries.
"""

import os
import sys
import json
import base64
import requests
from pathlib import Path
from tempfile import TemporaryDirectory
from datetime import datetime

from datasets import Dataset, Audio, load_dataset
from huggingface_hub import HfApi

# Configuration
POSTGREST_URL = os.environ.get("EARS_POSTGREST_URL", "http://centurion:30433")
POSTGREST_JWT = os.environ["EARS_POSTGREST_JWT"]
HF_TOKEN = os.environ["HF_TOKEN"]
HF_REPO = "heiertech/ears-asr-dataset"
STATE_FILE = Path.home() / ".local/state/ears/hf-sync-state.json"


def load_state():
    """Load sync state (last synced ID)."""
    if STATE_FILE.exists():
        return json.loads(STATE_FILE.read_text())
    return {"last_id": 0}


def save_state(state):
    """Save sync state."""
    STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
    STATE_FILE.write_text(json.dumps(state))


def fetch_new_samples(last_id: int) -> list:
    """Fetch samples with ID > last_id from PostgREST."""
    headers = {"Authorization": f"Bearer {POSTGREST_JWT}"}
    response = requests.get(
        f"{POSTGREST_URL}/voice_samples",
        headers=headers,
        params={"id": f"gt.{last_id}", "order": "id.asc"}
    )
    response.raise_for_status()
    return response.json()


def sync_dataset():
    """Sync new samples to HF dataset."""
    state = load_state()
    last_id = state["last_id"]

    print(f"Fetching samples with id > {last_id}...")
    new_samples = fetch_new_samples(last_id)

    if not new_samples:
        print("No new samples to sync.")
        return

    print(f"Found {len(new_samples)} new samples")

    # Load existing dataset to append
    try:
        existing_ds = load_dataset(HF_REPO, token=HF_TOKEN, split="train")
        print(f"Existing dataset has {len(existing_ds)} samples")
    except Exception as e:
        print(f"Could not load existing dataset: {e}")
        existing_ds = None

    # Create new samples dataset
    with TemporaryDirectory() as tmpdir:
        audio_paths = []
        transcriptions = []

        for sample in new_samples:
            audio_data = base64.b64decode(sample["audio_base64"])
            audio_path = Path(tmpdir) / f"sample_{sample['id']}.wav"
            audio_path.write_bytes(audio_data)

            audio_paths.append(str(audio_path))
            transcriptions.append(sample["transcription"])

        new_ds = Dataset.from_dict({
            "audio": audio_paths,
            "transcription": transcriptions,
        })
        new_ds = new_ds.cast_column("audio", Audio(sampling_rate=16000))

        # Concatenate with existing if available
        if existing_ds is not None:
            from datasets import concatenate_datasets
            combined_ds = concatenate_datasets([existing_ds, new_ds])
        else:
            combined_ds = new_ds

        print(f"Pushing {len(combined_ds)} total samples to {HF_REPO}...")
        combined_ds.push_to_hub(
            HF_REPO,
            private=True,
            token=HF_TOKEN,
            commit_message=f"Add {len(new_samples)} new samples (ids {new_samples[0]['id']}-{new_samples[-1]['id']})"
        )

    # Update state
    state["last_id"] = new_samples[-1]["id"]
    state["last_sync"] = datetime.now().isoformat()
    save_state(state)

    print(f"Synced! Last ID: {state['last_id']}")


if __name__ == "__main__":
    sync_dataset()
