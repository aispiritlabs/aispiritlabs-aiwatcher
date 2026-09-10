from typing import Any

import pytest

from aiwatcher_agentic.response_parser import ParsedResponse, ResponseParser, _contains_thinking
from aiwatcher_agentic.structured_output import StructuredOutputProto
from aiwatcher_agentic.tools import Toolset, Toolsets


def echo(text: str) -> str:
    return f"ok:{text}"


def make_parser(
    with_tools: bool = True,
    structured_output: StructuredOutputProto | None = None,
) -> ResponseParser:
    toolsets = Toolsets([Toolset([echo])]) if with_tools else Toolsets()
    return ResponseParser(toolsets, structured_output=structured_output)


def test_parse_detects_tool_call() -> None:
    parser = make_parser()
    result = parser.parse('{"name":"echo","parameters":{"text":"hi"}}')

    assert result.tool_detected is True
    assert len(result.tool_calls) == 1
    assert result.tool_calls[0] == ("echo", {"text": "hi"})
    assert result.content == '{"name":"echo","parameters":{"text":"hi"}}'


def test_parse_plain_text() -> None:
    parser = make_parser()
    result = parser.parse("Hello, world!")

    assert result.tool_detected is False
    assert result.tool_calls == []
    assert result.content == "Hello, world!"
    assert result.reasoning == ""


def test_parse_detects_thinking() -> None:
    parser = make_parser(with_tools=False)
    response = "<thinking>Let me think about this...</thinking>The answer is 42."
    result = parser.parse(response)

    assert result.reasoning == response
    assert result.content == response
    assert result.tool_detected is False


def test_tool_detection_takes_priority_over_thinking() -> None:
    parser = make_parser()
    response = '{"name":"echo","parameters":{"text":"<thinking>test</thinking>"}}'
    result = parser.parse(response)

    assert result.tool_detected is True
    assert len(result.tool_calls) == 1
    assert result.reasoning == ""


def test_parse_with_no_tools_returns_plain() -> None:
    parser = make_parser(with_tools=False)
    result = parser.parse("just text")

    assert result.tool_detected is False
    assert result.tool_calls == []
    assert result.content == "just text"


def test_contains_thinking_true() -> None:
    assert _contains_thinking("<thinking>reasoning here</thinking>") is True


def test_contains_thinking_false() -> None:
    assert _contains_thinking("plain text") is False


def test_parsed_response_is_frozen() -> None:
    response = ParsedResponse(content="test")
    with pytest.raises(AttributeError):
        response.content = "changed"  # type: ignore[misc]


class FakeStructuredOutput:
    def __init__(self, marker: str = "STRUCTURED:"):
        self._marker = marker

    def is_structured(self, response: str) -> bool:
        return response.startswith(self._marker)

    def parse(self, response: str) -> str:
        return response.removeprefix(self._marker)

    def response_format(self) -> dict[str, Any] | None:
        return None


def test_parse_structured_output() -> None:
    structured = FakeStructuredOutput()
    parser = make_parser(with_tools=False, structured_output=structured)
    result = parser.parse("STRUCTURED:parsed content")

    assert result.content == "parsed content"
    assert result.tool_detected is False
    assert result.reasoning == ""


def test_tool_detection_takes_priority_over_structured_output() -> None:
    structured = FakeStructuredOutput(marker='{"name":')
    parser = make_parser(structured_output=structured)
    result = parser.parse('{"name":"echo","parameters":{"text":"x"}}')

    assert result.tool_detected is True
    assert len(result.tool_calls) == 1
