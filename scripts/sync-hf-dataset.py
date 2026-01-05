#!/usr/bin/env bash
#
# Sync ears voice samples from PostgreSQL to Hugging Face dataset
# Uses audiofolder format for efficient incremental updates
#

set -euo pipefail

# Load required environment variables
source ~/.secrets
source ~/.config/ears/hooks/.env

exec python3 << 'PYTHON'
"""
Sync voice samples from PostgreSQL to Hugging Face dataset.
Uses audiofolder format - only uploads new files without re-downloading.
"""

import os
import json
import csv
import base64
import requests
from pathlib import Path
from tempfile import TemporaryDirectory
from datetime import datetime
from io import StringIO

from huggingface_hub import HfApi, hf_hub_download

# Configuration
POSTGREST_URL = os.environ.get("EARS_POSTGREST_URL", "http://centurion:30433")
POSTGREST_JWT = os.environ["EARS_POSTGREST_JWT"]
HF_TOKEN = os.environ["HF_TOKEN"]
HF_REPO = "heiertech/ears-asr-dataset"
STATE_FILE = Path.home() / ".local/state/ears/hf-sync-state.json"


def load_state():
    if STATE_FILE.exists():
        return json.loads(STATE_FILE.read_text())
    return {"last_id": 0, "next_file_num": 1}


def save_state(state):
    STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
    STATE_FILE.write_text(json.dumps(state))


def fetch_new_samples(last_id: int) -> list:
    headers = {"Authorization": f"Bearer {POSTGREST_JWT}"}
    response = requests.get(
        f"{POSTGREST_URL}/voice_samples",
        headers=headers,
        params={"id": f"gt.{last_id}", "order": "id.asc"}
    )
    response.raise_for_status()
    return response.json()


def get_existing_metadata(api: HfApi) -> list:
    """Download and parse existing metadata.csv"""
    try:
        metadata_path = hf_hub_download(
            repo_id=HF_REPO,
            filename="data/metadata.csv",
            repo_type="dataset",
            token=HF_TOKEN
        )
        with open(metadata_path) as f:
            reader = csv.DictReader(f)
            return list(reader)
    except Exception:
        return []


def sync_dataset():
    state = load_state()
    last_id = state["last_id"]
    next_file_num = state.get("next_file_num", 1)

    print(f"Fetching samples with id > {last_id}...")
    new_samples = fetch_new_samples(last_id)

    if not new_samples:
        print("No new samples to sync.")
        return

    print(f"Found {len(new_samples)} new samples")

    api = HfApi(token=HF_TOKEN)

    # Get existing metadata
    existing_metadata = get_existing_metadata(api)
    if existing_metadata:
        # Find highest file number
        for row in existing_metadata:
            try:
                num = int(row["file_name"].replace(".wav", ""))
                next_file_num = max(next_file_num, num + 1)
            except ValueError:
                pass

    print(f"Starting from file number {next_file_num}")

    with TemporaryDirectory() as tmpdir:
        tmpdir = Path(tmpdir)

        new_metadata_rows = []
        files_to_upload = []

        for i, sample in enumerate(new_samples):
            file_num = next_file_num + i
            file_name = f"{file_num:04d}.wav"

            # Decode and save audio
            audio_data = base64.b64decode(sample["audio_base64"])
            audio_path = tmpdir / file_name
            audio_path.write_bytes(audio_data)

            files_to_upload.append((str(audio_path), f"data/{file_name}"))

            new_metadata_rows.append({
                "file_name": file_name,
                "transcription": sample["transcription"],
                "duration_ms": sample.get("duration_ms", 0),
                "sample_rate": sample.get("sample_rate", 16000)
            })

            print(f"  Prepared {file_name}: {sample['transcription'][:40]}...")

        # Combine metadata
        all_metadata = existing_metadata + new_metadata_rows

        # Write updated metadata.csv
        metadata_path = tmpdir / "metadata.csv"
        with open(metadata_path, "w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=["file_name", "transcription", "duration_ms", "sample_rate"])
            writer.writeheader()
            writer.writerows(all_metadata)

        # Upload new audio files
        print(f"Uploading {len(files_to_upload)} audio files...")
        for local_path, repo_path in files_to_upload:
            api.upload_file(
                path_or_fileobj=local_path,
                path_in_repo=repo_path,
                repo_id=HF_REPO,
                repo_type="dataset",
                commit_message=f"Add {Path(repo_path).name}"
            )

        # Upload updated metadata
        print("Uploading updated metadata.csv...")
        api.upload_file(
            path_or_fileobj=str(metadata_path),
            path_in_repo="data/metadata.csv",
            repo_id=HF_REPO,
            repo_type="dataset",
            commit_message=f"Update metadata: add {len(new_samples)} samples (ids {new_samples[0]['id']}-{new_samples[-1]['id']})"
        )

    # Update state
    state["last_id"] = new_samples[-1]["id"]
    state["next_file_num"] = next_file_num + len(new_samples)
    state["last_sync"] = datetime.now().isoformat()
    save_state(state)

    print(f"Synced {len(new_samples)} samples! Last ID: {state['last_id']}")


if __name__ == "__main__":
    sync_dataset()
PYTHON
