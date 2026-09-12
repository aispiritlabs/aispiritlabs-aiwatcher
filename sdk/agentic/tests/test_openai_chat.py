"""The chat-completions adapter: what it sends, and what it refuses to return.

Every case runs over a transport that records the request and replays a
canned body, because what is being tested is the translation — profile into
request, response into `ModelResponse` — and not HTTP.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

import orjson
import pytest

from aiwatcher_agentic.exceptions import ModelResponseError
from aiwatcher_agentic.openai_chat import LLAMA_CPP, OPENAI, ModelProfile, OpenAIChatModel


class Recording:
    """A transport that answers with `reply` and keeps what it was asked."""

    def __init__(self, reply: dict[str, Any]) -> None:
        self.reply = reply
        self.url = ""
        self.body: dict[str, Any] = {}
        self.headers: dict[str, str] = {}

    def __call__(self, url: str, body: bytes, headers: Mapping[str, str], timeout: float) -> bytes:
        self.url = url
        self.body = orjson.loads(body)
        self.headers = dict(headers)
        return orjson.dumps(self.reply)


def completion(**message: Any) -> dict[str, Any]:
    return {
        "id": "req-1",
        "model": "served-name",
        "choices": [{"finish_reason": "stop", "message": {"content": "answer", **message}}],
        "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10},
    }


def test_a_string_prompt_becomes_one_user_turn_and_the_profile_sets_the_knobs() -> None:
    transport = Recording(completion())
    model = OpenAIChatModel("http://llm:8080/v1/", "animica", transport=transport)

    result = model.response("find panels")

    assert transport.url == "http://llm:8080/v1/chat/completions"
    assert transport.body["messages"] == [{"role": "user", "content": "find panels"}]
    assert transport.body["model"] == "animica"
    assert transport.body["temperature"] == OPENAI.temperature
    assert transport.body["max_tokens"] == OPENAI.max_tokens
    assert "authorization" not in transport.headers
    assert result.text == "answer"
    assert result.model == "served-name"
    assert result.request_id == "req-1"
    assert (result.prompt_tokens, result.completion_tokens, result.total_tokens) == (7, 3, 10)


def test_a_message_list_is_sent_as_it_is_and_the_caller_outranks_the_profile() -> None:
    transport = Recording(completion())
    model = OpenAIChatModel(
        "http://llm:8080/v1", "animica", transport=transport, api_key="secret", profile=LLAMA_CPP
    )
    turns = [{"role": "system", "content": "rules"}, {"role": "user", "content": "find panels"}]

    model.response(turns, max_tokens=256)

    assert transport.body["messages"] == turns
    assert transport.body["max_tokens"] == 256
    assert transport.body["chat_template_kwargs"] == {"enable_thinking": False}
    assert transport.headers["authorization"] == "Bearer secret"


def test_a_quantised_profile_requires_every_property_without_touching_the_callers_schema() -> None:
    """A schema reused across calls must come back the way it went in."""
    transport = Recording(completion())
    model = OpenAIChatModel("http://llm/v1", "animica", transport=transport, profile=LLAMA_CPP)
    schema = {
        "type": "object",
        "properties": {"queries": {"type": "array"}, "offers": {"type": "array"}},
        "required": ["queries"],
    }
    response_format = {"type": "json_schema", "json_schema": {"name": "offers", "schema": schema}}

    model.response("find panels", response_format=response_format)

    sent = transport.body["response_format"]["json_schema"]["schema"]
    assert sent["required"] == ["queries", "offers"]
    assert schema["required"] == ["queries"]


def test_a_generic_profile_leaves_an_optional_field_optional() -> None:
    transport = Recording(completion())
    model = OpenAIChatModel("http://llm/v1", "gpt", transport=transport)
    response_format = {
        "type": "json_schema",
        "json_schema": {"schema": {"properties": {"a": {}, "b": {}}, "required": ["a"]}},
    }

    model.response("q", response_format=response_format)

    assert transport.body["response_format"]["json_schema"]["schema"]["required"] == ["a"]


def test_a_native_tool_call_is_translated_into_the_portable_one() -> None:
    """The endpoint's shape stops here; the rest of the SDK reads one shape."""
    transport = Recording(
        completion(
            content=None,
            tool_calls=[
                {
                    "function": {
                        "name": "search_catalog",
                        "arguments": '{"query": "fence panels"}',
                    }
                }
            ],
        )
    )
    tool = {"type": "function", "function": {"name": "search_catalog"}}
    model = OpenAIChatModel("http://llm/v1", "animica", transport=transport, tools=(tool,))

    result = model.response("find panels")

    assert transport.body["tools"] == [tool]
    assert transport.body["tool_choice"] == "auto"
    assert transport.body["parallel_tool_calls"] is False
    assert orjson.loads(result.text) == {
        "name": "search_catalog",
        "parameters": {"query": "fence panels"},
    }


@pytest.mark.parametrize(
    ("message", "match"),
    [
        ({"content": ""}, "empty answer"),
        ({"content": "   "}, "empty answer"),
        (
            {"content": None, "tool_calls": [{"function": {"name": "a", "arguments": "{}"}}] * 2},
            "one turn carries one call",
        ),
        (
            {"content": None, "tool_calls": [{"function": {"name": "a", "arguments": "not json"}}]},
            "not JSON",
        ),
        (
            {"content": None, "tool_calls": [{"function": {"name": "", "arguments": "{}"}}]},
            "no name or no argument object",
        ),
        (
            {"content": None, "tool_calls": [{"function": {"name": "a", "arguments": "[1]"}}]},
            "no name or no argument object",
        ),
    ],
)
def test_an_answer_that_is_not_a_completion_is_refused(message: dict[str, Any], match: str) -> None:
    transport = Recording(completion(**message))
    model = OpenAIChatModel("http://llm/v1", "animica", transport=transport)

    with pytest.raises(ModelResponseError, match=match):
        model.response("find panels")


def test_a_truncated_completion_is_a_failure_unless_the_profile_allows_it() -> None:
    """Half a JSON answer parses often enough to be worse than none."""
    reply = completion()
    reply["choices"][0]["finish_reason"] = "length"
    transport = Recording(reply)

    with pytest.raises(ModelResponseError, match="ran out of output tokens"):
        OpenAIChatModel("http://llm/v1", "animica", transport=transport).response("q")

    lenient = ModelProfile(name="lenient", fail_on_length=False)
    result = OpenAIChatModel(
        "http://llm/v1", "animica", transport=transport, profile=lenient
    ).response("q")
    assert result.finish_reason == "length"


def test_a_body_with_no_choices_names_the_model_rather_than_raising_a_key_error() -> None:
    transport = Recording({"error": {"message": "overloaded"}})
    model = OpenAIChatModel("http://llm/v1", "animica", transport=transport)

    with pytest.raises(ModelResponseError, match="no completion to read"):
        model.response("q")


def test_provider_side_citations_and_cost_survive_onto_the_response() -> None:
    reply = completion(annotations=[{"type": "url_citation", "url": "https://example.org"}, "junk"])
    reply["usage"] |= {"cost": 0.0021, "prompt_tokens_details": {"cached_tokens": 4}}
    transport = Recording(reply)
    model = OpenAIChatModel("https://openrouter.ai/api/v1", "sonoma", transport=transport)

    result = model.response("q")

    assert result.cost == 0.0021
    assert result.cached_tokens == 4
    assert result.annotations == ({"type": "url_citation", "url": "https://example.org"},)
    assert model.annotations == result.annotations


def test_the_model_lends_itself_and_owns_no_transport() -> None:
    transport = Recording(completion())
    model = OpenAIChatModel("http://llm/v1", "animica", transport=transport)

    with model.session() as lent:
        assert lent is model
    model.close()

    assert model.response("q").text == "answer"
