from __future__ import annotations

from dataclasses import dataclass, field

import structlog

from aiwatcher_agentic.structured_output import StructuredOutputProto
from aiwatcher_agentic.tools import ToolCall, Toolsets

logger = structlog.get_logger(__name__)


@dataclass(frozen=True)
class ParsedResponse:
    content: str
    reasoning: str = ""
    tool_calls: list[ToolCall] = field(default_factory=list)
    tool_detected: bool = False


def _contains_thinking(response: str) -> bool:
    return "<thinking>" in response


class ResponseParser:
    def __init__(
        self,
        toolsets: Toolsets,
        structured_output: StructuredOutputProto | None = None,
    ) -> None:
        self._toolsets = toolsets
        self._structured_output = structured_output

    def parse(self, raw_response: str) -> ParsedResponse:
        if self._toolsets.detect_tool(raw_response):
            tool_call = self._toolsets._coerce_tool_call(
                raw_response, repairer=self._toolsets._json_repairer
            )
            if tool_call is not None:
                logger.info("detected_tool", tool_definition=tool_call)
                return ParsedResponse(
                    content=raw_response,
                    tool_calls=[tool_call],
                    tool_detected=True,
                )

        if self._structured_output and self._structured_output.is_structured(raw_response):
            return ParsedResponse(
                content=self._structured_output.parse(raw_response),
            )

        if _contains_thinking(raw_response):
            logger.info("detected_thinking", response=raw_response)
            return ParsedResponse(
                content=raw_response,
                reasoning=raw_response,
            )

        return ParsedResponse(content=raw_response)
