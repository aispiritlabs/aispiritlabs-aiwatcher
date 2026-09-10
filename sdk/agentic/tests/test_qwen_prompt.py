from aiwatcher_agentic.prompts import QwenPromptBuilder


def test_qwen_prompt_builds_native_template_string() -> None:
    prompt = QwenPromptBuilder(system_prompt="SYSTEM {tools}")
    rendered = prompt.build_prompt("Użytkownik pyta", toolsets=None)

    assert isinstance(rendered, str)
    assert rendered.startswith("<|im_start|>system\nSYSTEM")
    assert "<|im_start|>user\nUżytkownik pyta\n<|im_end|>" in rendered
    assert rendered.endswith("<|im_start|>assistant\n")


def test_qwen_prompt_converts_native_gemma_turn_history() -> None:
    prompt = QwenPromptBuilder(system_prompt="SYSTEM {tools}")
    gemma_history = (
        "<start_of_turn>user\nHej\n<end_of_turn>\n"
        "<start_of_turn>model\nCześć\n<end_of_turn>\n"
        "<start_of_turn>user\nDalej\n<end_of_turn>"
    )

    rendered = prompt.build_prompt(gemma_history, toolsets=None)

    assert "<start_of_turn>" not in rendered
    assert "<end_of_turn>" not in rendered
    assert "<|im_start|>assistant\nCześć\n<|im_end|>" in rendered
    assert "<|im_start|>user\nDalej\n<|im_end|>" in rendered


def test_qwen_prompt_does_not_double_wrap_when_system_prompt_is_already_turn_formatted() -> None:
    raw_system = (
        "<|im_start|>system\nINSTRUKCJE\n<|im_end|>\n<|im_start|>assistant\nWzorzec\n<|im_end|>"
    )
    prompt = QwenPromptBuilder(system_prompt=raw_system)
    rendered = prompt.build_prompt("hej", toolsets=None)
    assert isinstance(rendered, str)

    assert "<|im_start|>system\n<|im_start|>system" not in rendered
    assert "<|im_end|>\n<|im_end|>" not in rendered
    assert rendered.count("<|im_start|>assistant") == 2


def test_qwen_prompt_keeps_non_tools_placeholders_literal() -> None:
    prompt = QwenPromptBuilder(system_prompt="SYSTEM {tools}\n{name}\n{vault_path}")

    rendered = prompt.build_prompt("hej", toolsets=None)
    assert isinstance(rendered, str)

    assert "{name}" in rendered
    assert "{vault_path}" in rendered
