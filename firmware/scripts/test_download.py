"""Pinned downloads retry transient failures and never retry a wrong file into success."""
import hashlib
import http.client
import io
import unittest
from unittest.mock import patch
import urllib.error

import download

DATA = b'pinned fixture'
SHA = hashlib.sha256(DATA).hexdigest()


def http_error(code):
    return urllib.error.HTTPError('https://example.invalid/a', code, 'status', {}, None)


class Fetch(unittest.TestCase):
    def fetch(self, *responses, attempts=5):
        pauses = []
        with patch.object(download.urllib.request, 'urlopen', side_effect=list(responses)) as opener:
            try:
                return download.fetch('https://example.invalid/a', SHA, attempts=attempts, sleep=pauses.append), opener.call_count, pauses
            except Exception as error:
                error.calls, error.pauses = opener.call_count, pauses
                raise

    def test_transient_failures_are_retried_with_growing_pauses(self):
        data, calls, pauses = self.fetch(
            urllib.error.URLError('EOF occurred in violation of protocol'),
            http.client.IncompleteRead(b'pinned'), http_error(503), io.BytesIO(DATA))
        self.assertEqual((data, calls, pauses), (DATA, 4, [2.0, 4.0, 8.0]))

    def test_a_missing_file_fails_at_once(self):
        with self.assertRaises(urllib.error.HTTPError) as raised:
            self.fetch(http_error(404), io.BytesIO(DATA))
        self.assertEqual(raised.exception.calls, 1)

    def test_a_wrong_file_is_not_retried(self):
        with self.assertRaises(download.DigestMismatch) as raised:
            self.fetch(io.BytesIO(b'replaced'), io.BytesIO(DATA))
        self.assertEqual(raised.exception.calls, 1)

    def test_the_last_transient_error_surfaces_after_all_attempts(self):
        with self.assertRaises(urllib.error.URLError) as raised:
            self.fetch(*[urllib.error.URLError('down')] * 3, attempts=3)
        self.assertEqual((raised.exception.calls, raised.exception.pauses), (3, [2.0, 4.0]))


if __name__ == '__main__':
    unittest.main()
