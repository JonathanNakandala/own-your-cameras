"""Pinned downloads that survive transient network failures.

Retries connection errors, truncated bodies, HTTP 429 and 5xx with a growing pause; any other HTTP
status and a wrong SHA-256 fail at once, so a moved or replaced file is never retried into success.
"""
import hashlib
import http.client
import time
import urllib.error
import urllib.request

ATTEMPTS = 5
FIRST_PAUSE = 2.0
HEADERS = {'User-Agent': 'own-your-cameras-build'}


class DigestMismatch(ValueError):
    pass


def retryable(error):
    if isinstance(error, urllib.error.HTTPError):
        return error.code == 429 or error.code >= 500
    return isinstance(error, (urllib.error.URLError, http.client.HTTPException, OSError))


def fetch(url, sha256, *, attempts=ATTEMPTS, pause=FIRST_PAUSE, sleep=time.sleep):
    """The body of url, checked against sha256."""
    for attempt in range(1, attempts + 1):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS), timeout=60) as response:
                data = response.read()
            break
        except Exception as error:
            if attempt == attempts or not retryable(error):
                raise
            print(f'download: {url}: {error}; retry {attempt}/{attempts - 1} in {pause:.0f} s', flush=True)
            sleep(pause)
            pause *= 2
    if hashlib.sha256(data).hexdigest() != sha256:
        raise DigestMismatch(f'SHA-256 mismatch: {url}')
    return data
