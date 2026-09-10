"""Typed usage tracking and budget enforcement for agent runs."""

from __future__ import annotations

from dataclasses import dataclass, field

from aiwatcher_agentic.exceptions import UsageLimitExceeded


@dataclass(slots=True)
class RequestUsage:
    """Token usage for a single model request."""

    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    latency_ms: float = 0.0
    model: str = ""
    finish_reason: str = ""


@dataclass(slots=True)
class RunUsage:
    """Accumulated usage across an entire agent run (multiple model requests)."""

    requests: int = 0
    tool_calls: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    total_tokens: int = 0
    total_latency_ms: float = 0.0
    request_usages: list[RequestUsage] = field(default_factory=list)

    def add(self, usage: RequestUsage) -> None:
        self.requests += 1
        self.input_tokens += usage.prompt_tokens
        self.output_tokens += usage.completion_tokens
        self.total_tokens += usage.total_tokens
        self.total_latency_ms += usage.latency_ms
        self.request_usages.append(usage)

    def add_tool_calls(self, count: int) -> None:
        self.tool_calls += count


@dataclass(frozen=True, slots=True)
class UsageLimits:
    """Budget limits for an agent run.

    Set a field to ``None`` to disable that limit.
    """

    request_limit: int | None = None
    tool_calls_limit: int | None = None
    input_tokens_limit: int | None = None
    output_tokens_limit: int | None = None
    total_tokens_limit: int | None = None

    def check_before_request(self, usage: RunUsage) -> None:
        """Check limits before making a model request. Raises ``UsageLimitExceeded``."""
        if self.request_limit is not None and usage.requests >= self.request_limit:
            raise UsageLimitExceeded(
                f"Request limit exceeded: {usage.requests}/{self.request_limit}"
            )
        self._check_tokens(usage)

    def check_after_request(self, usage: RunUsage) -> None:
        """Check limits after a model request completes. Raises ``UsageLimitExceeded``."""
        self._check_tokens(usage)

    def _check_tokens(self, usage: RunUsage) -> None:
        if self.input_tokens_limit is not None and usage.input_tokens > self.input_tokens_limit:
            raise UsageLimitExceeded(
                f"Input token limit exceeded: {usage.input_tokens}/{self.input_tokens_limit}"
            )
        if self.output_tokens_limit is not None and usage.output_tokens > self.output_tokens_limit:
            raise UsageLimitExceeded(
                f"Output token limit exceeded: {usage.output_tokens}/{self.output_tokens_limit}"
            )
        if self.total_tokens_limit is not None and usage.total_tokens > self.total_tokens_limit:
            raise UsageLimitExceeded(
                f"Total token limit exceeded: {usage.total_tokens}/{self.total_tokens_limit}"
            )
        if self.tool_calls_limit is not None and usage.tool_calls > self.tool_calls_limit:
            raise UsageLimitExceeded(
                f"Tool calls limit exceeded: {usage.tool_calls}/{self.tool_calls_limit}"
            )


UNLIMITED = UsageLimits()
