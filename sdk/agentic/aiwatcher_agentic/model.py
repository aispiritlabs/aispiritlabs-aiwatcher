"""What an agent needs from a model, and nothing about where the model runs.

An `Agent` asks a source for a model for one unit of work and hands it a
prompt. How that model was loaded — an MLX process on a Mac, an
OpenAI-compatible endpoint, a vLLM server — is an adapter's business, and the
adapters stay with the application that chooses one (`providers`, in
`ai_spirit_agent`). This module is the port they implement: the response, the
loaded model, and the source that lends it. Importing it imports no model
stack, which is the point of it being here.
"""

from __future__ import annotations

from contextlib import AbstractContextManager
from dataclasses import dataclass, field
from typing import Any, Protocol, runtime_checkable


@dataclass(frozen=True, slots=True)
class ModelResponse:
    text: str
    model: str = ""
    request_id: str = ""
    finish_reason: str = ""
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    latency_ms: float = 0.0
    #: What the call cost, when the endpoint prices it. A local model does not.
    cost: float | None = None
    #: Prompt tokens served from the provider's cache. Read by the aiwatcher
    #: tracer, which looks for this name on whatever response it is handed.
    cached_tokens: int = 0
    #: Sources a provider-side tool attached to the answer — OpenRouter's web
    #: search returns `url_citation` entries here.
    annotations: tuple[dict[str, Any], ...] = field(default=())


@runtime_checkable
class TextModel(Protocol):
    """A loaded model that answers a prompt."""

    def response(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse: ...

    def close(self) -> None: ...


class ModelSource(Protocol):
    """Lends an agent a model for one unit of work.

    ``None`` is a model that could not be loaded. A source that knows why may
    also answer ``get_load_error(name) -> str | None``; an agent reads it when
    it is there and says "not available" when it is not, so a test double
    needs only this one method.
    """

    def session(self, name: str = "model") -> AbstractContextManager[TextModel | None]: ...
