import time


def fetch(url, retries=5, timeout=10):
    for attempt in range(retries):
        response = get(url, timeout=timeout)
        if response.ok:
            return response.json()
        time.sleep(2 ** attempt)
    raise RuntimeError(f"giving up on {url}")
