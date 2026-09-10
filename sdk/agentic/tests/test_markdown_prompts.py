"""Markdown prompt templates, and the four ways a caller can get them wrong."""

from __future__ import annotations

from pathlib import Path

import pytest

from aiwatcher_agentic.prompts import MarkdownPromptBuilder


def _builder(tmp_path: Path, **templates: str) -> MarkdownPromptBuilder:
    for name, body in templates.items():
        path = tmp_path / name.replace("__", "/")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")
    return MarkdownPromptBuilder(tmp_path)


def test_declared_values_are_rendered_in_both_spacings(tmp_path: Path) -> None:
    builder = _builder(tmp_path, **{"sample.md": "Hello {{ name }} — {{count}}."})

    assert builder.build("sample.md", name="Planner", count=2) == "Hello Planner — 2."


def test_a_template_in_a_subdirectory_is_named_by_its_path(tmp_path: Path) -> None:
    builder = _builder(tmp_path, **{"floor_plan__system.pl.md": "Jesteś {{ role }}."})

    assert builder.build("floor_plan/system.pl.md", role="rysownikiem") == "Jesteś rysownikiem."


@pytest.mark.parametrize(
    ("values", "message"),
    [({}, "Missing prompt values"), ({"name": "Planner", "extra": 1}, "Unexpected prompt values")],
)
def test_a_value_set_that_does_not_match_the_template_is_refused(
    tmp_path: Path,
    values: dict[str, object],
    message: str,
) -> None:
    builder = _builder(tmp_path, **{"sample.md": "Hello {{ name }}."})

    with pytest.raises(ValueError, match=message):
        builder.build("sample.md", **values)


def test_single_braces_belong_to_the_layer_above_and_are_left_alone(tmp_path: Path) -> None:
    builder = _builder(tmp_path, **{"sample.md": "You may call {tools} for {{ goal }}."})

    assert builder.build("sample.md", goal="drafting") == "You may call {tools} for drafting."


def test_a_name_that_climbs_out_of_the_directory_is_refused(tmp_path: Path) -> None:
    builder = MarkdownPromptBuilder(tmp_path)

    with pytest.raises(ValueError, match="Invalid prompt template name"):
        builder.build("../secret.md")


def test_a_missing_or_empty_template_says_which_one(tmp_path: Path) -> None:
    builder = _builder(tmp_path, **{"blank.md": "   \n"})

    with pytest.raises(ValueError, match=r"not found: 'nope\.md'"):
        builder.build("nope.md")
    with pytest.raises(ValueError, match=r"empty: 'blank\.md'"):
        builder.build("blank.md")


def test_a_template_is_read_from_disk_once(tmp_path: Path) -> None:
    builder = _builder(tmp_path, **{"sample.md": "Hello {{ name }}."})
    assert builder.build("sample.md", name="first") == "Hello first."

    (tmp_path / "sample.md").write_text("Rewritten {{ name }}.", encoding="utf-8")

    assert builder.build("sample.md", name="second") == "Hello second."
