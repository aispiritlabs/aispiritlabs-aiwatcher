import pytest

from aiwatcher_agentic.exceptions import ToolValidationError
from aiwatcher_agentic.tools import Toolset, Toolsets


def add_note(note_name: str, note: str) -> str:
    return f"{note_name}:{note}"


def test_execute_raises_for_missing_required_parameter() -> None:
    toolset = Toolset([add_note])

    with pytest.raises(ToolValidationError) as raised:
        toolset.execute("add_note", {"note_name": "Pizza"})

    assert "missing required parameters" in str(raised.value)
    assert "note" in str(raised.value)


def test_run_tool_returns_error_message_for_invalid_parameters() -> None:
    toolsets = Toolsets([Toolset([add_note])])

    result = toolsets.run_tool(("add_note", {"note_name": "Pizza"}))
    assert result is not None

    assert "missing required parameters" in result.output
    assert "add_note" in result.output
    assert result.retry is True


def test_run_tool_accepts_raw_payload_and_returns_tool_definition() -> None:
    toolsets = Toolsets([Toolset([add_note])])

    result = toolsets.run_tool(
        '{"name":"add_note","parameters":{"note_name":"Pizza","note":"Pepperoni"}}'
    )
    assert result is not None

    assert result.tool_call == ("add_note", {"note_name": "Pizza", "note": "Pepperoni"})
    assert result.output == "Pizza:Pepperoni"
