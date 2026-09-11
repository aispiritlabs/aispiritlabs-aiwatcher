from __future__ import annotations

import asyncio
import dataclasses
import hashlib
import threading
import time
from collections.abc import AsyncGenerator, Callable, Sequence
from dataclasses import dataclass, field
from typing import Any
from uuid import uuid4

import structlog

from aiwatcher_agentic.capabilities import AbstractCapability, CombinedCapability, HookContext
from aiwatcher_agentic.exceptions import DEFAULT_RETRY, ModelRetry, RetryPolicy
from aiwatcher_agentic.history import History
from aiwatcher_agentic.memory import InMemory, Memory
from aiwatcher_agentic.message import (
    AssistantMessage,
    ImagePart,
    Message,
    SystemMessage,
    UserMessage,
)
from aiwatcher_agentic.model import ModelResponse, ModelSource
from aiwatcher_agentic.prompts import PromptBuilder
from aiwatcher_agentic.response_parser import ResponseParser
from aiwatcher_agentic.structured_output import StructuredOutputProto
from aiwatcher_agentic.tools import (
    JsonRepairer,
    Tool,
    ToolCall,
    ToolContext,
    ToolRunResult,
    Toolset,
    Toolsets,
    build_chat_tools,
)
from aiwatcher_agentic.tracer import LLMTracer, NoopLLMTracer
from aiwatcher_agentic.usage import UNLIMITED, RequestUsage, RunUsage, UsageLimits
from aiwatcher_agentic.workflow.trace import TraceSnapshot

logger = structlog.get_logger(__name__)

ToolOutput = str

#: What ``run(images=…)`` takes. A path or URL for the local vision providers,
#: an :class:`~aiwatcher_agentic.message.ImagePart` for anything that goes over HTTP.
type ImageInput = str | ImagePart | dict[str, Any] | bytes


def _stringify_content(content: Any) -> str:
    if isinstance(content, str):
        return content
    if content is None:
        return ""
    return str(content)


@dataclass(frozen=True)
class PromptSnapshot:
    text: str
    prompt_name: str | None = None
    prompt_hash: str = ""
    tool_schema: list[dict[str, Any]] = field(default_factory=list)


@dataclass(frozen=True)
class Context:
    add_history_to_context: bool = True
    max_history_messages: int = 20
    observability_tags: dict[str, Any] | None = None


@dataclass(frozen=True)
class PromptContext:
    message: str
    history_text: str = ""
    memory_context: str = ""


@dataclass(frozen=True)
class PromptArtifacts:
    prompt: str | list[dict[str, str]]
    system_prompt_text: str
    prompt_name: str | None = None
    prompt_hash: str = ""
    tool_schema: list[dict[str, Any]] = field(default_factory=list)
    #: The registry version of the template ``prompt_name`` read — not
    #: ``prompt_hash``, which is the rendered prompt and names no version.
    prompt_version: str | None = None

    def __contains__(self, item: object) -> bool:
        return isinstance(self.prompt, str) and isinstance(item, str) and item in self.prompt

    def __str__(self) -> str:
        if isinstance(self.prompt, str):
            return self.prompt
        return str(self.prompt)


class AgentResult:
    def __init__(
        self,
        content: Any,
        *,
        reasoning: str = "",
        tool_calls: list[ToolCall] | None = None,
        usage: dict[str, Any] | None = None,
        request_usage: RequestUsage | None = None,
        response_text: str | None = None,
        trace: TraceSnapshot | None = None,
        run_id: str | None = None,
        prompt_snapshot: PromptSnapshot | None = None,
        attempt_no: int | None = None,
        loop_iteration: int | None = None,
        model_provider: str = "",
        generation_config_hash: str = "",
    ) -> None:
        self.content = content
        self.reasoning = reasoning
        self.tool_calls: list[ToolCall] = tool_calls or []
        self.usage = usage or {}
        self.request_usage = request_usage or RequestUsage()
        self.response_text = (
            response_text if response_text is not None else _stringify_content(content)
        )
        self.trace = trace
        self.run_id = run_id
        self.prompt_snapshot = prompt_snapshot
        self.attempt_no = attempt_no
        self.loop_iteration = loop_iteration
        self.model_provider = model_provider
        self.generation_config_hash = generation_config_hash

    @property
    def content_text(self) -> str:
        return self.response_text

    @property
    def tool_call(self) -> ToolCall | None:
        if not self.tool_calls:
            return None
        return self.tool_calls[0]

    @property
    def trace_id(self) -> str | None:
        if self.trace is None or not self.trace.trace_id:
            return None
        return self.trace.trace_id

    @property
    def session_id(self) -> str | None:
        if self.trace is None or not self.trace.session_id:
            return None
        return self.trace.session_id

    @property
    def span_id(self) -> str | None:
        if self.trace is None or not self.trace.span_id:
            return None
        return self.trace.span_id

    @property
    def parent_span_id(self) -> str | None:
        if self.trace is None or not self.trace.parent_span_id:
            return None
        return self.trace.parent_span_id

    @property
    def span_name(self) -> str | None:
        if self.trace is None or not self.trace.span_name:
            return None
        return self.trace.span_name

    @property
    def span_type(self) -> str | None:
        if self.trace is None or not self.trace.span_type:
            return None
        return self.trace.span_type


def _message_preview(message: str, max_length: int = 128) -> str:
    if len(message) <= max_length:
        return message
    return f"{message[:max_length]}..."


def _serialize_tool_schema(toolsets: Toolsets) -> list[dict[str, Any]]:
    schema: list[dict[str, Any]] = []
    for toolset in toolsets:
        for tool in toolset.tools:
            schema.append(
                {
                    "name": tool.name,
                    "description": tool.doc,
                    "arguments": [dict(argument) for argument in tool.args],
                }
            )
    return schema


def _build_trace_messages(
    message_text: str,
    prompt_artifacts: PromptArtifacts,
) -> list[dict[str, Any]]:
    if isinstance(prompt_artifacts.prompt, list):
        return [dict(message) for message in prompt_artifacts.prompt]
    messages: list[dict[str, Any]] = []
    if prompt_artifacts.system_prompt_text:
        messages.append({"role": "system", "content": prompt_artifacts.system_prompt_text})
    messages.append({"role": "user", "content": message_text})
    return messages


class Agent:
    def __init__(
        self,
        model_provider: ModelSource,
        *,
        prompt_builder: PromptBuilder,
        context: Context | None = None,
        memory: Memory | None = None,
        history: History | None = None,
        tools: Sequence[Callable[..., Any] | Tool] | None = None,
        toolsets: Toolsets | Sequence[Toolset] | None = None,
        json_repairer: JsonRepairer | None = None,
        structured_output: StructuredOutputProto | None = None,
        tracer: LLMTracer | None = None,
        usage_limits: UsageLimits | None = None,
        retry_policy: RetryPolicy = DEFAULT_RETRY,
        capabilities: Sequence[AbstractCapability] | None = None,
    ) -> None:
        self._model_provider = model_provider
        self._toolsets = Toolsets.from_sources(
            tools=tools,
            toolsets=toolsets,
            json_repairer=json_repairer,
        )
        self._prompt_builder = prompt_builder
        self._context = context or Context(add_history_to_context=True)
        self._memory = memory or InMemory()
        self._history = history or History()
        self._agent_id = str(uuid4())
        self._tracer = tracer or NoopLLMTracer()
        self._structured_output = structured_output
        self._response_parser = ResponseParser(self._toolsets, structured_output)
        self._usage_limits = usage_limits or UNLIMITED
        self._run_usage = RunUsage()
        self._retry_policy = retry_policy
        self._capability = CombinedCapability(capabilities) if capabilities else None
        # An agent owns conversation history and usage counters, so a turn has to
        # be atomic. Re-entrant because a turn may call back into run() (tool retries).
        self._turn_lock = threading.RLock()

    @property
    def turn_lock(self) -> threading.RLock:
        """Serialises turns on this agent.

        Callers that need several agent operations to happen as one turn — for
        example ``clear_history()`` immediately followed by ``run()`` — must hold
        this for the whole sequence.
        """
        return self._turn_lock

    @property
    def history(self) -> History:
        return self._history

    @history.setter
    def history(self, value: History) -> None:
        self._history = value

    @property
    def toolsets(self) -> Toolsets:
        return self._toolsets

    @property
    def context(self) -> Context:
        return self._context

    @property
    def agent_id(self) -> str:
        return self._agent_id

    @property
    def system_prompt(self) -> PromptBuilder:
        return self._prompt_builder

    @system_prompt.setter
    def system_prompt(self, value: PromptBuilder) -> None:
        self._prompt_builder = value

    def _render_system_prompt(
        self,
        hook_context: HookContext | None = None,
    ) -> tuple[str, list[dict[str, Any]]]:
        tool_prompt = (
            "\n".join(
                self._prompt_builder.tools_instruction(toolset)
                for toolset in self._toolsets
                if toolset.tools
            )
            if self._toolsets
            else ""
        )
        system_template = self._prompt_builder.system_prompt or ""
        rendered = system_template.replace("{tools}", tool_prompt).strip()
        if self._capability is not None and hook_context is not None:
            extra_instructions = self._capability.get_instructions(hook_context)
            if extra_instructions:
                rendered = "\n\n".join(part for part in [rendered, extra_instructions] if part)
        return rendered, _serialize_tool_schema(self._toolsets)

    def _gather_context(
        self,
        message_text: str,
        current_context: Context,
        run_turns: Sequence[Message] = (),
    ) -> PromptContext:
        stored = (
            self._history.conversation_text(current_context.max_history_messages)
            if current_context.add_history_to_context
            else ""
        )
        replayed = "\n".join(turn.as_turn() for turn in run_turns)
        history_text = "\n".join(part for part in [stored, replayed] if part)
        # Keep the staged memory API active so custom Memory implementations can
        # adopt the future prompt-context contract before prompt injection lands.
        memory_context = self._memory.summary()
        return PromptContext(
            message=message_text,
            history_text=history_text,
            memory_context=memory_context,
        )

    @staticmethod
    def _render_input_turn(message: str | Message) -> str:
        if isinstance(message, Message):
            as_turn = getattr(message, "as_turn", None)
            if callable(as_turn):
                turn: str = as_turn()
                return turn
            return str(message)
        return UserMessage(str(message)).as_turn()

    def _build_prompt(
        self,
        prompt_context: PromptContext,
        current_message: str | Message | None = None,
        hook_context: HookContext | None = None,
    ) -> PromptArtifacts:
        rendered_input_turn = self._render_input_turn(
            prompt_context.message if current_message is None else current_message
        )
        memory_turn = (
            SystemMessage(f"Remembered context:\n{prompt_context.memory_context}").as_turn()
            if prompt_context.memory_context
            else ""
        )
        message_with_history = "\n".join(
            turn
            for turn in [
                memory_turn,
                prompt_context.history_text,
                rendered_input_turn,
            ]
            if turn
        )
        system_prompt_text, tool_schema = self._render_system_prompt(hook_context)
        prompt = self._prompt_builder.build_prompt(
            message_with_history,
            system_prompt=system_prompt_text,
            toolsets=self._toolsets,
        )
        prompt_name = getattr(self._prompt_builder, "external_prompt_name", None)
        prompt_version = getattr(self._prompt_builder, "external_prompt_version", None)
        return PromptArtifacts(
            prompt=prompt,
            system_prompt_text=system_prompt_text,
            prompt_name=prompt_name or None,
            prompt_hash=hashlib.sha256(system_prompt_text.encode("utf-8")).hexdigest(),
            tool_schema=tool_schema,
            prompt_version=prompt_version or None,
        )

    def _call_model(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse:
        with self._model_provider.session("model") as model:
            if model is None:
                load_error: str | None = None
                get_load_error = getattr(self._model_provider, "get_load_error", None)
                if callable(get_load_error):
                    load_error = get_load_error("model")
                if load_error:
                    raise RuntimeError(f"Model is not available for inference: {load_error}")
                raise RuntimeError("Model is not available for inference.")
            return model.response(prompt, **kwargs)

    @staticmethod
    def _request_usage_from_response(model_response: ModelResponse) -> RequestUsage:
        return RequestUsage(
            prompt_tokens=model_response.prompt_tokens,
            completion_tokens=model_response.completion_tokens,
            total_tokens=model_response.total_tokens,
            latency_ms=model_response.latency_ms,
            model=model_response.model,
            finish_reason=model_response.finish_reason,
        )

    @staticmethod
    def _build_validation_retry_message(
        original_message: str,
        *,
        error_text: str,
        previous_response: str,
    ) -> str:
        return "\n".join(
            [
                "Your previous response could not be accepted.",
                f"Validation error: {error_text}",
                "Previous response:",
                previous_response,
                "",
                "Respond again to the original request below with a corrected answer.",
                "Original request:",
                original_message,
            ]
        )

    @staticmethod
    def _build_correction_message(error_text: str) -> str:
        """The correction turn, for a conversation that already shows the answer."""
        return "\n".join(
            [
                "Your previous response could not be accepted.",
                f"Validation error: {error_text}",
                "",
                "Correct it and answer the original request again.",
            ]
        )

    def _to_result(
        self,
        model_response: ModelResponse,
        *,
        run_id: str,
        prompt_artifacts: PromptArtifacts,
        trace: TraceSnapshot | None,
        generation_config_hash: str,
    ) -> AgentResult:
        parsed = self._response_parser.parse(model_response.text)
        req_usage = self._request_usage_from_response(model_response)
        return AgentResult(
            content=parsed.content,
            reasoning=parsed.reasoning,
            tool_calls=list(parsed.tool_calls),
            usage={
                "prompt_tokens": model_response.prompt_tokens,
                "completion_tokens": model_response.completion_tokens,
                "total_tokens": model_response.total_tokens,
                "latency_ms": model_response.latency_ms,
                "model": model_response.model,
                "finish_reason": model_response.finish_reason,
            },
            request_usage=req_usage,
            response_text=model_response.text,
            trace=trace,
            run_id=run_id,
            prompt_snapshot=PromptSnapshot(
                text=prompt_artifacts.system_prompt_text,
                prompt_name=prompt_artifacts.prompt_name,
                prompt_hash=prompt_artifacts.prompt_hash,
                tool_schema=list(prompt_artifacts.tool_schema),
            ),
            model_provider=str(getattr(self._model_provider, "_model_provider_type", "")),
            generation_config_hash=generation_config_hash,
        )

    def _store_history(self, message: str | Message, response_content: str) -> None:
        if isinstance(message, Message):
            self._history.add(message)
        else:
            self._history.add(UserMessage(message))
        self._history.add(AssistantMessage(response_content))

    @property
    def run_usage(self) -> RunUsage:
        return self._run_usage

    def make_hook_context(
        self,
        run_id: str | None = None,
        *,
        turn: int = 0,
        metadata: dict[str, Any] | None = None,
    ) -> HookContext:
        return HookContext(
            agent_id=self._agent_id,
            run_id=run_id or "",
            turn=turn,
            metadata=metadata or {},
        )

    def run_tool(
        self,
        payload: Any,
        *,
        tool_context: ToolContext | None = None,
        tracer: LLMTracer | None = None,
        run_id: str | None = None,
        turn: int = 0,
    ) -> ToolRunResult | None:
        hook_context = self.make_hook_context(run_id, turn=turn)
        return self._toolsets.run_tool(
            payload,
            tool_context=tool_context,
            tracer=tracer,
            capability=self._capability,
            hook_context=hook_context,
        )

    def run(
        self,
        message: str | Message,
        ctx: Context | None = None,
        *,
        images: ImageInput | Sequence[ImageInput] | None = None,
        history: Sequence[Message] = (),
    ) -> AgentResult:
        """`history` is prior turns for this run only — a repair attempt replaying
        what the model already answered — and is not added to the agent's own
        conversation."""
        with self._turn_lock:
            return self._run_locked(message, ctx, images=images, history=history)

    def _run_locked(
        self,
        message: str | Message,
        ctx: Context | None,
        *,
        images: ImageInput | Sequence[ImageInput] | None,
        history: Sequence[Message] = (),
    ) -> AgentResult:
        current_context = ctx or self._context
        message_text = str(message)
        run_id = str(uuid4())
        self._run_usage = RunUsage()

        model_kwargs: dict[str, Any] = {}
        if images is not None:
            # `image` is the name the local vision providers take; the HTTP path
            # reads either, so one key serves both.
            model_kwargs["image"] = images
        if self._structured_output is not None:
            response_format = self._structured_output.response_format()
            if response_format is not None:
                model_kwargs["response_format"] = response_format

        attempt = 0
        current_message: str | Message = message
        # Turns this run replays but does not own: the caller's `history`, plus
        # the rejected answers when the retry policy keeps them.
        run_turns: list[Message] = list(history)
        hook_ctx = self.make_hook_context(run_id, turn=attempt)

        with self._tracer.agent(
            name="agent.run",
            agent_id=self._agent_id,
            input=_message_preview(message_text),
            attributes={
                "run_id": run_id,
                "history_enabled": current_context.add_history_to_context,
            },
        ) as span:
            try:
                while True:
                    self._usage_limits.check_before_request(self._run_usage)
                    hook_ctx = self.make_hook_context(run_id, turn=attempt)

                    current_message_text = str(current_message)
                    prompt_context = self._gather_context(
                        current_message_text, current_context, run_turns
                    )
                    prompt_artifacts = self._build_prompt(
                        prompt_context,
                        current_message,
                        hook_context=hook_ctx,
                    )

                    if self._capability is not None:
                        # `replace`, not a rebuild, for the reason given below.
                        prompt_artifacts = dataclasses.replace(
                            prompt_artifacts,
                            prompt=self._capability.before_model_request(
                                prompt_artifacts.prompt, hook_ctx
                            ),
                        )

                    trace_messages = _build_trace_messages(current_message_text, prompt_artifacts)
                    chat_tools = build_chat_tools(prompt_artifacts.tool_schema)

                    model_response = self._tracer.llm(
                        name="llm-call",
                        model=getattr(self._model_provider, "_model_name", None) or "",
                        messages=trace_messages,
                        tools=chat_tools,
                        extra_attributes={
                            "agentic.prompt_hash": prompt_artifacts.prompt_hash,
                            "agentic.tool_count": len(prompt_artifacts.tool_schema),
                            **(
                                {"agentic.prompt_name": prompt_artifacts.prompt_name}
                                if prompt_artifacts.prompt_name
                                else {}
                            ),
                            **(
                                {"agentic.prompt_version": prompt_artifacts.prompt_version}
                                if prompt_artifacts.prompt_version
                                else {}
                            ),
                        },
                        invoke=lambda artifacts=prompt_artifacts: self._call_model(
                            artifacts.prompt, **model_kwargs
                        ),
                    )

                    if self._capability is not None:
                        # `replace`, not a rebuild: a hook rewrites the text and
                        # nothing else, and a field it has never heard of (cost,
                        # citations) must survive it.
                        model_response = dataclasses.replace(
                            model_response,
                            text=self._capability.after_model_request(
                                model_response.text, hook_ctx
                            ),
                        )

                    self._run_usage.add(self._request_usage_from_response(model_response))

                    try:
                        result = self._to_result(
                            model_response,
                            run_id=run_id,
                            prompt_artifacts=prompt_artifacts,
                            trace=self._tracer.current_trace,
                            generation_config_hash=hashlib.sha256(
                                repr(sorted(model_kwargs.items())).encode("utf-8")
                            ).hexdigest(),
                        )
                    except ModelRetry as error:
                        if self._capability is not None:
                            self._capability.on_error(error, hook_ctx)
                        self._usage_limits.check_after_request(self._run_usage)
                        if not self._retry_policy.should_retry(attempt, error):
                            raise
                        attempt += 1
                        if self._retry_policy.keep_history:
                            run_turns.append(UserMessage(current_message_text))
                            run_turns.append(AssistantMessage(model_response.text))
                            current_message = self._build_correction_message(str(error))
                        else:
                            current_message = self._build_validation_retry_message(
                                message_text,
                                error_text=str(error),
                                previous_response=model_response.text,
                            )
                        if wait := self._retry_policy.wait_time(attempt):
                            time.sleep(wait)
                        continue

                    if result.tool_calls:
                        self._run_usage.add_tool_calls(len(result.tool_calls))
                    self._usage_limits.check_after_request(self._run_usage)

                    span.update(
                        output={
                            "content": result.content_text[:200],
                            "usage.requests": self._run_usage.requests,
                            "usage.input_tokens": self._run_usage.input_tokens,
                            "usage.output_tokens": self._run_usage.output_tokens,
                            "usage.total_tokens": self._run_usage.total_tokens,
                            "usage.tool_calls": self._run_usage.tool_calls,
                            "usage.latency_ms": self._run_usage.total_latency_ms,
                            "usage.retry_attempts": attempt,
                        }
                    )
                    self._store_history(message, result.content_text)
                    return result
            except Exception as error:
                if self._capability is not None and not isinstance(error, ModelRetry):
                    self._capability.on_error(error, hook_ctx)
                raise

    async def arun(
        self,
        message: str | Message,
        ctx: Context | None = None,
        *,
        images: ImageInput | Sequence[ImageInput] | None = None,
        history: Sequence[Message] = (),
    ) -> AgentResult:
        return await asyncio.to_thread(self.run, message, ctx, images=images, history=history)

    async def astream(
        self,
        message: str | Message,
        ctx: Context | None = None,
        *,
        images: ImageInput | Sequence[ImageInput] | None = None,
        history: Sequence[Message] = (),
    ) -> AsyncGenerator[dict[str, str | AgentResult]]:
        result = await self.arun(message, ctx, images=images, history=history)
        yield {"type": "result", "result": result}

    def clear_history(self) -> None:
        with self._turn_lock:
            self._history.clear()
