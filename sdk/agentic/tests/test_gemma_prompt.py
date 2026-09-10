from aiwatcher_agentic.message import Message, SystemMessage, UserMessage
from aiwatcher_agentic.prompts import GemmaPromptBuilder


def test_system_and_user_message_render_native_gemma_turns() -> None:
    system_turn = SystemMessage("Instrukcje").as_turn()
    user_turn = UserMessage("Hej").as_turn()

    assert system_turn == "<start_of_turn>model\nInstrukcje\n<end_of_turn>"
    assert user_turn == "<start_of_turn>user\nHej\n<end_of_turn>"


def test_gemma_prompt_builds_native_template_string() -> None:
    prompt = GemmaPromptBuilder(system_prompt="SYSTEM {tools}")
    rendered = prompt.build_prompt("Użytkownik pyta", toolsets=None)

    assert isinstance(rendered, str)
    assert rendered.startswith("<start_of_turn>model\nSYSTEM")
    assert "<start_of_turn>user\nUżytkownik pyta\n<end_of_turn>" in rendered
    assert rendered.endswith("<start_of_turn>model\n")


def test_gemma_prompt_reads_user_message_text_via_get_text() -> None:
    prompt = GemmaPromptBuilder(system_prompt="SYSTEM {tools}")
    rendered = prompt.build_prompt(UserMessage("Treść od usera"), toolsets=None)

    assert "<start_of_turn>user\nTreść od usera\n<end_of_turn>" in rendered


def test_message_structural_get_text_is_role_neutral() -> None:
    prompt = GemmaPromptBuilder(system_prompt="SYSTEM {tools}")
    message = Message(structural_message={"role": "user", "content": "Treść"})
    rendered = prompt.build_prompt(message, toolsets=None)

    assert "<start_of_turn>user\nTreść\n<end_of_turn>" in rendered
    assert "role:" not in rendered
    assert "content:" not in rendered


def test_gemma_prompt_does_not_double_wrap_when_system_prompt_is_already_turn_formatted() -> None:
    raw_system = (
        "<start_of_turn>model\nINSTRUKCJE\n<end_of_turn>\n"
        "<start_of_turn>user\nWzorzec\n<end_of_turn>"
    )
    prompt = GemmaPromptBuilder(system_prompt=raw_system)
    rendered = prompt.build_prompt("[START_PERSONALIZATION]", toolsets=None)
    assert isinstance(rendered, str)

    assert "<start_of_turn>model\n<start_of_turn>model" not in rendered
    assert "<end_of_turn>\n<end_of_turn>" not in rendered
    assert rendered.count("<start_of_turn>user") == 2


def test_gemma_prompt_keeps_non_tools_placeholders_literal() -> None:
    prompt = GemmaPromptBuilder(system_prompt="SYSTEM {tools}\n{name}\n{vault_path}")

    rendered = prompt.build_prompt("hej", toolsets=None)

    assert "{name}" in rendered
    assert "{vault_path}" in rendered
