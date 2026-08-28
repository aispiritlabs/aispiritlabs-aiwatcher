from __future__ import annotations

import unittest
from typing import Any

from aiwatcher_sdk import AiwatcherClient, HttpTransport


class _FlushableTransport:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []
        self.flushes = 0

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def flush(self) -> None:
        self.flushes += 1

    def close(self) -> None:
        return None


class HttpTransportTests(unittest.TestCase):
    def test_flush_waits_until_queued_events_are_posted(self) -> None:
        posted: list[dict[str, Any]] = []
        transport = HttpTransport(
            "http://aiwatcher.invalid",
            batch_size=64,
            flush_interval=60.0,
            timeout=1.0,
        )
        transport._post = lambda batch: posted.extend(batch)  # type: ignore[method-assign]
        try:
            transport.send([{"event_id": "first"}, {"event_id": "second"}])

            transport.flush()

            self.assertEqual([event["event_id"] for event in posted], ["first", "second"])
        finally:
            transport.close()


class AiwatcherClientTests(unittest.TestCase):
    def test_flush_delegates_to_flushable_transport(self) -> None:
        transport = _FlushableTransport()
        client = AiwatcherClient(service="test", transport=transport)

        client.flush()

        self.assertEqual(transport.flushes, 1)


if __name__ == "__main__":
    unittest.main()
