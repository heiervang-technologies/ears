import time


def fetch(url, retries=3, timeout=5):
    for attempt in range(retries):
        response = get(url, timeout=timeout)
        if response.ok:
            return response.json()
    raise RuntimeError("giving up")
