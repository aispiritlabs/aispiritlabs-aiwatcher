"""The runtime is handed its tracer and settings, and reaches for no backend itself.

What it read from `ai_spirit_agent` before it moved — MLflow tracing, a
pydantic settings object from `.env`, the OpenAI-compatible client — is a port
now, or a hook, and the application that has them hands them in.
"""

from __future__ import annotations

import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import pytest

from aiwatcher_agentic.runtime.config import DistributedSettings, RuntimeConfig, RuntimeSettings
from aiwatcher_agentic.runtime.distributed.discovery import AgenticServiceDiscovery
from aiwatcher_agentic.runtime.distributed.in_memory_transport import (
    InMemoryServiceRegistry,
    InMemoryTransport,
)
from aiwatcher_agentic.runtime.distributed.service import DistributedService
from aiwatcher_agentic.runtime.runtime import AgenticRuntime
from aiwatcher_agentic.tracer import NoopLLMTracer
from aiwatcher_agentic.workflow.description import Description
from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    Message,
    RecordedMessageMetadata,
    UserMessage,
)

#: What an application brings, and this distribution must not.
BACKENDS = {
    "agentic",
    "agentic_runtime",
    "aiwatcher_sdk",
    "httpx",
    "mlflow",
    "providers",
    "pydantic",
    "pydantic_settings",
    "registry",
    "torch",
    "transformers",
}


class Router:
    def route(self, message: str, available_workflows_summary: str) -> str:
        raise AssertionError("a targeted turn names its agent and never asks the router")

    def start(self) -> str:
        return ""

    def close(self) -> None:
        pass


class Echo:
    description = Description("echo", "says it back", ())
    inputs: Sequence[str] = ("UserMessage",)

    def handle(self, message: Message) -> str:
        assert isinstance(message, UserMessage)
        return f"echo: {message.data.text}"

    def close(self) -> None:
        pass


class ApplicationSettings:
    """Settings the way an application keeps them: plain, mutable attributes."""

    event_store_path: str | None = None
    message_store_path: str | None = None
    message_store_batch_size = 64
    message_store_flush_interval_seconds = 0.05
    message_stream_inline_bytes = 4096
    message_stream_chunk_bytes = 4096
    distributed_prefix = "lab"
    agent_liveness_ttl_seconds = 20.0
    distributed_max_delivery_attempts = 3
    distributed_retry_min_idle_ms = 5_000
    distributed_require_event_store = True
    model_name = "whatever the application runs"


def stores_under(root: Path) -> RuntimeConfig:
    return RuntimeConfig(
        event_store_path=str(root / "events.db"), message_store_path=str(root / "messages.db")
    )


def test_importing_the_runtime_imports_no_backend() -> None:
    probe = (
        "import sys, aiwatcher_agentic.runtime, aiwatcher_agentic.runtime.hosted, "
        "aiwatcher_agentic.runtime.distributed\n"
        f"print(' '.join(sorted({{m.split('.')[0] for m in sys.modules}} & {BACKENDS!r})))"
    )

    imported = subprocess.run(  # noqa: S603 - our own interpreter, running our own probe
        [sys.executable, "-c", probe], capture_output=True, text=True, check=True
    )

    assert imported.stdout.split() == []


def test_a_runtime_answers_a_turn_with_nothing_but_what_it_was_handed(tmp_path: Path) -> None:
    runtime = AgenticRuntime(
        workflows=[Echo()], router=Router(), settings=stores_under(tmp_path), tracer=NoopLLMTracer()
    )
    try:
        reply = runtime.handle(
            UserMessage(
                data=ConversationData(role="user", text="hello"),
                metadata=RecordedMessageMetadata(source="test", target="echo"),
            )
        )
    finally:
        runtime.close()

    assert reply == "echo: hello"


def test_the_stores_default_to_the_working_directory_and_never_beside_the_code(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)

    AgenticRuntime(workflows=[Echo()], router=Router()).close()

    written = {path.name for path in (tmp_path / ".data").iterdir()}
    assert {"message_stream.sqlite3", "workflow_event_store.sqlite3"} <= written
    installed = Path(sys.modules[AgenticRuntime.__module__].__file__ or "").parents
    assert not any((parent / "data").exists() for parent in installed[:5])


def test_an_application_configures_its_model_client_once_its_workflows_are_registered(
    tmp_path: Path,
) -> None:
    class Configuring(AgenticRuntime):
        def _configure_providers(self) -> None:
            self.configured_with = sorted(self.workflows)

    runtime = Configuring(workflows=[Echo()], router=Router(), settings=stores_under(tmp_path))
    runtime.close()

    assert runtime.configured_with == ["echo"]


def test_an_application_s_own_settings_satisfy_both_ports() -> None:
    # Checked by mypy: these assignments are the claim.
    ours: RuntimeSettings = ApplicationSettings()
    distributed: DistributedSettings = ApplicationSettings()
    defaults: RuntimeSettings = RuntimeConfig()

    assert ours.message_store_batch_size == defaults.message_store_batch_size
    assert distributed.distributed_prefix == "lab"


def test_discovery_holds_every_service_to_the_settings_it_was_handed() -> None:
    def ignore(message: Message, discovery: Any) -> Sequence[Message]:
        return ()

    strict = AgenticServiceDiscovery(
        InMemoryTransport(), InMemoryServiceRegistry(), settings=ApplicationSettings()
    )
    with pytest.raises(RuntimeError, match="EVENT_STORE_PATH"):
        strict.create_service("planner", capabilities=("plan",), handler=ignore)

    default = AgenticServiceDiscovery(InMemoryTransport(), InMemoryServiceRegistry())
    service = default.create_service("planner", capabilities=("plan",), handler=ignore)
    service.close()
    assert isinstance(service, DistributedService)
