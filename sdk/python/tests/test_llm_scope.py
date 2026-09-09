"""What an `llm` scope puts on the wire, and what it deliberately does not."""

from __future__ import annotations

import unittest
from dataclasses import dataclass
from typing import Any

from aiwatcher_sdk import AiwatcherClient


class RecordingTransport:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def close(self) -> None:
        return None


@dataclass(frozen=True)
class FakeVersion:
    """Shaped like `prompts.PromptVersion` without importing that half."""

    name: str
    version_id: str
    text: str


VERSION_ID = "c" * 64


def emitted(**scope: Any) -> list[dict[str, Any]]:
    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    with client.run("run-1") as run, run.agent("floor-plan") as agent, agent.llm(**scope) as call:
        call.usage(prompt_tokens=10, completion_tokens=3)
    return transport.events


def payload_of(events: list[dict[str, Any]], event_type: str) -> dict[str, Any]:
    for event in events:
        if event["event_type"] == event_type:
            return dict(event["data"])
    raise AssertionError(f"no {event_type} in {[e['event_type'] for e in events]}")


class LlmScopeTests(unittest.TestCase):
    def test_request_settings_ride_on_both_the_start_and_the_end(self) -> None:
        events = emitted(model="claude-opus-5", temperature=0.2, top_p=0.95, max_tokens=4096)

        for event_type in ("llm.started", "llm.completed"):
            payload = payload_of(events, event_type)
            self.assertEqual(payload["temperature"], 0.2)
            self.assertEqual(payload["top_p"], 0.95)
            self.assertEqual(payload["max_tokens"], 4096)

    def test_a_prompt_version_object_becomes_the_two_reference_fields(self) -> None:
        version = FakeVersion(
            name="planner.floor-plan.system",
            version_id=VERSION_ID,
            text="You are a floor plan vectorizer.",
        )

        payload = payload_of(emitted(model="m", prompt=version), "llm.started")

        self.assertEqual(payload["prompt_name"], "planner.floor-plan.system")
        self.assertEqual(payload["prompt_version"], VERSION_ID)

    def test_the_prompt_text_never_reaches_the_wire(self) -> None:
        version = FakeVersion(
            name="planner.floor-plan.system",
            version_id=VERSION_ID,
            text="You are a floor plan vectorizer.",
        )

        events = emitted(model="m", prompt=version)

        for event in events:
            self.assertNotIn("You are a floor plan", str(event))

    def test_a_pair_and_a_bare_id_are_both_accepted(self) -> None:
        pair = payload_of(emitted(model="m", prompt=("a-prompt", VERSION_ID)), "llm.started")
        self.assertEqual(pair["prompt_name"], "a-prompt")
        self.assertEqual(pair["prompt_version"], VERSION_ID)

        # An id with no name identifies the text; it simply cannot be resolved.
        bare = payload_of(emitted(model="m", prompt=VERSION_ID), "llm.started")
        self.assertEqual(bare["prompt_version"], VERSION_ID)
        self.assertNotIn("prompt_name", bare)

    def test_an_explicit_field_wins_over_the_prompt_argument(self) -> None:
        payload = payload_of(
            emitted(model="m", prompt=VERSION_ID, prompt_version="d" * 64),
            "llm.started",
        )

        self.assertEqual(payload["prompt_version"], "d" * 64)


if __name__ == "__main__":
    unittest.main()
