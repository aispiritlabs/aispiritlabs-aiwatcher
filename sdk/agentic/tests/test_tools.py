import pytest

from aiwatcher_agentic.tools import Tool, Toolset, Toolsets


def greet(name: str, excited: bool = False) -> str:
    if excited:
        return f"Hello, {name}!"
    return f"Hello, {name}"


def update_personalization(name: str, vault_path: str) -> str:
    return f"{name}:{vault_path}"


def ping() -> str:
    return "pong"


def test_tool_extracts_required_and_optional_parameters() -> None:
    tool = Tool(greet)

    assert tool.name == "greet"
    assert tool.required_parameters == {"name"}
    assert tool.all_parameters == {"name", "excited"}
    assert tool.call({"name": "Mateusz", "excited": True}) == "Hello, Mateusz!"


def test_toolset_execute_and_has_tool() -> None:
    toolset = Toolset([greet, ping])

    assert toolset.has_tool("greet")
    assert toolset.has_tool("ping")
    assert not toolset.has_tool("missing")
    assert toolset.execute("greet", {"name": "Ala"}) == "Hello, Ala"

    with pytest.raises(ValueError, match="not found"):
        toolset.execute("missing", {})


def test_toolsets_detect_and_run_json_tool_payload() -> None:
    toolsets = Toolsets([Toolset([update_personalization])])
    payload = '{"name":"update_personalization","parameters":{"name":"Ala","vault_path":"/v"}}'

    assert toolsets.detect_tool(payload)
    run = toolsets.run_tool(payload)

    assert run is not None
    assert run.tool_call == ("update_personalization", {"name": "Ala", "vault_path": "/v"})
    assert run.output == "Ala:/v"


def test_toolsets_returns_not_found_message_for_unknown_tool() -> None:
    toolsets = Toolsets([Toolset([ping])])

    run = toolsets.run_tool('{"name":"missing","parameters":{}}')

    assert run is not None
    assert run.output == "Error: tool 'missing' does not exist."
