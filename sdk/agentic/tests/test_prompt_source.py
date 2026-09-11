"""A builder named by ``external_prompt_name`` reads from a source the application names."""

from __future__ import annotations

import hashlib
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


def test_a_named_prompt_carries_the_version_the_registry_names_its_text_by() -> None:
    builder = QwenPromptBuilder(external_prompt_name="sage", prompt_source=lambda name: "Be wise.")

    assert builder.external_prompt_version == hashlib.sha256(b"Be wise.").hexdigest()


def test_a_prompt_nobody_read_by_name_names_no_version() -> None:
    written = ChatPromptBuilder(system_prompt="SYSTEM")
    given = ChatPromptBuilder(
        system_prompt="SYSTEM", external_prompt_name="sage", prompt_source=lambda name: "read"
    )

    assert written.external_prompt_version is None
    assert given.external_prompt_version is None


def test_a_prompt_changed_after_it_was_read_is_no_longer_that_version() -> None:
    edited = ChatPromptBuilder(external_prompt_name="sage", prompt_source=lambda name: "Be wise.")
    edited.system_prompt = "Be brief."
    renamed = ChatPromptBuilder(external_prompt_name="sage", prompt_source=lambda name: "Be wise.")
    renamed.external_prompt_name = "chat"

    assert edited.external_prompt_version is None
    assert renamed.external_prompt_version is None
