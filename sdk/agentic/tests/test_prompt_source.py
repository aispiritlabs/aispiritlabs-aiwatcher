"""A builder named by ``external_prompt_name`` reads from a source the application names."""

from __future__ import annotations

from collections.abc import Generator

import pytest

from aiwatcher_agentic.prompts import ChatPromptBuilder, QwenPromptBuilder, use_prompt_source


@pytest.fixture(autouse=True)
def no_application_source() -> Generator[None, None, None]:
    use_prompt_source(None)
    yield
    use_prompt_source(None)


def test_a_named_prompt_is_read_from_the_source_the_builder_was_handed() -> None:
    builder = QwenPromptBuilder(
        external_prompt_name="greeting", prompt_source=lambda name: f"<{name}>"
    )

    assert builder.system_prompt == "<greeting>"
    assert builder.external_prompt_name == "greeting"


def test_a_named_prompt_is_read_from_the_application_s_source_otherwise() -> None:
    use_prompt_source(lambda name: f"registry: {name}")

    assert ChatPromptBuilder(external_prompt_name="sage").system_prompt == "registry: sage"


def test_the_builder_s_own_source_wins_over_the_application_s() -> None:
    use_prompt_source(lambda name: "application")

    builder = ChatPromptBuilder(external_prompt_name="sage", prompt_source=lambda name: "own")

    assert builder.system_prompt == "own"


def test_a_named_prompt_with_nowhere_to_read_it_from_is_refused_by_name() -> None:
    with pytest.raises(LookupError, match=r"'sage'.*use_prompt_source"):
        ChatPromptBuilder(external_prompt_name="sage")


def test_text_given_directly_is_never_looked_up() -> None:
    def refuse(name: str) -> str:
        raise AssertionError(f"looked up {name!r}")

    builder = ChatPromptBuilder(
        system_prompt="SYSTEM", external_prompt_name="sage", prompt_source=refuse
    )

    assert builder.system_prompt == "SYSTEM"
