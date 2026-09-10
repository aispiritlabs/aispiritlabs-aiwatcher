"""Turn atomicity on ``Agent``.

An agent owns conversation history and usage counters, so a turn has to be
atomic: two concurrent ``run()`` calls on the same agent would interleave
history writes and reset each other's ``RunUsage``. The lock is per agent, so
distinct agents must still run in parallel.
"""

from __future__ import annotations

import threading
import time
from collections.abc import Generator
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager

import pytest

from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.history import History
from aiwatcher_agentic.model import ModelResponse
from aiwatcher_agentic.prompts import GemmaPromptBuilder

HOLD_SECONDS = 0.02


class ConcurrencyProbeModel:
    """Records how many callers are inside ``response`` at the same time."""

    def __init__(self, hold: float = HOLD_SECONDS) -> None:
        self._hold = hold
        self._lock = threading.Lock()
        self.active = 0
        self.max_active = 0
        self.calls = 0

    def response(self, prompt: str | list[dict[str, str]], **_kwargs: object) -> ModelResponse:
        del prompt
        with self._lock:
            self.active += 1
            self.max_active = max(self.max_active, self.active)
            self.calls += 1
        try:
            time.sleep(self._hold)
        finally:
            with self._lock:
                self.active -= 1
        return ModelResponse(
            text="ok",
            model="probe",
            prompt_tokens=1,
            completion_tokens=1,
            total_tokens=2,
            latency_ms=1.0,
        )

    def close(self) -> None:
        pass


class FakeProvider:
    def __init__(self, model: ConcurrencyProbeModel) -> None:
        self._model = model

    @contextmanager
    def session(self, name: str = "model") -> Generator[ConcurrencyProbeModel, None, None]:
        del name
        yield self._model


def _agent(model: ConcurrencyProbeModel, history: History | None = None) -> Agent:
    # A `session()` context manager is all `ModelSource` asks for.
    return Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        history=history,
    )


def test_concurrent_runs_on_one_agent_are_serialised() -> None:
    model = ConcurrencyProbeModel()
    agent = _agent(model)

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(lambda i: agent.run(f"pytanie {i}"), range(8)))

    assert model.calls == 8
    assert model.max_active == 1, "two turns overlapped inside one agent"
    assert all(result.content_text == "ok" for result in results)


def test_serialised_turns_record_every_exchange_in_history() -> None:
    model = ConcurrencyProbeModel()
    agent = _agent(model, history=History(max_turns=100))

    with ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(lambda i: agent.run(f"pytanie {i}"), range(8)))

    # One user turn and one assistant turn per run, nothing lost to a race.
    assert len(agent.history) == 16


def test_distinct_agents_are_not_blocked_by_each_other() -> None:
    # A single global lock would serialise the whole app; the lock is per agent.
    # The barrier makes this deterministic: if the four turns cannot be in
    # flight at once it breaks instead of merely running slowly.
    barrier = threading.Barrier(4, timeout=5)

    class RendezvousModel(ConcurrencyProbeModel):
        def response(self, prompt: str | list[dict[str, str]], **kwargs: object) -> ModelResponse:
            barrier.wait()
            return super().response(prompt, **kwargs)

    model = RendezvousModel(hold=0)
    agents = [_agent(model) for _ in range(4)]

    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(lambda a: a.run("pytanie"), agents))

    assert model.calls == 4
    assert not barrier.broken, "separate agents were serialised against each other"


def test_turn_lock_is_reentrant() -> None:
    # A turn may call back into run() (tool retries), so a plain Lock would
    # deadlock here.
    model = ConcurrencyProbeModel(hold=0)
    agent = _agent(model)

    with agent.turn_lock:
        result = agent.run("pytanie")

    assert result.content_text == "ok"
    assert model.calls == 1


def test_holding_the_turn_lock_lets_a_caller_group_several_operations() -> None:
    # The documented use: clear_history() immediately followed by run(), with
    # no other turn allowed in between.
    model = ConcurrencyProbeModel(hold=0)
    agent = _agent(model)
    agent.run("pierwsze")

    with agent.turn_lock:
        agent.clear_history()
        agent.run("drugie")

    assert len(agent.history) == 2


def test_clear_history_waits_for_an_in_flight_turn() -> None:
    model = ConcurrencyProbeModel(hold=0)
    agent = _agent(model)
    agent.run("pierwsze")

    held = threading.Event()
    release = threading.Event()
    cleared = threading.Event()

    def hold_a_turn() -> None:
        with agent.turn_lock:
            held.set()
            release.wait(timeout=5)

    def clear() -> None:
        agent.clear_history()
        cleared.set()

    holder = threading.Thread(target=hold_a_turn)
    clearer = threading.Thread(target=clear)
    holder.start()
    assert held.wait(timeout=5)

    clearer.start()
    assert not cleared.wait(timeout=0.2), "clear_history() ran during an open turn"
    assert len(agent.history) == 2

    release.set()
    holder.join(timeout=5)
    clearer.join(timeout=5)

    assert cleared.is_set()
    assert len(agent.history) == 0


def test_a_failing_turn_releases_the_lock() -> None:
    class ExplodingModel(ConcurrencyProbeModel):
        def response(self, prompt: str | list[dict[str, str]], **_kwargs: object) -> ModelResponse:
            self.calls += 1
            raise RuntimeError("model unavailable")

    model = ExplodingModel()
    agent = _agent(model)

    with pytest.raises(RuntimeError):
        agent.run("pytanie")

    # The lock must not be left held, or the agent is dead for the session.
    assert agent.turn_lock.acquire(timeout=1)
    agent.turn_lock.release()


def test_turn_lock_is_exposed_as_the_same_object_each_time() -> None:
    agent = _agent(ConcurrencyProbeModel(hold=0))

    assert agent.turn_lock is agent.turn_lock
