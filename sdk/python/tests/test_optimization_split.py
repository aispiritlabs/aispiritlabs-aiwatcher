"""The held-out split, which is the only evidence a promotion may rest on.

Nothing here reaches a network: the split is a pure function of a group, a salt
and a share, and that is the whole reason it can be checked without a server.
"""

from __future__ import annotations

from dataclasses import dataclass

import pytest

from aiwatcher_sdk.optimization import HELD_OUT_PERCENT, held_out_for, split_cases
from aiwatcher_sdk.prompts import RegistryError


@dataclass(frozen=True, slots=True)
class Golden:
    question: str
    phrasing: str


def goldens(*pairs: tuple[str, str]) -> tuple[Golden, ...]:
    return tuple(Golden(question=question, phrasing=phrasing) for question, phrasing in pairs)


def many(count: int, *, phrasings: int = 1) -> tuple[Golden, ...]:
    return goldens(
        *((f"q{index}", f"p{phrasing}") for index in range(count) for phrasing in range(phrasings))
    )


def test_every_phrasing_of_one_question_lands_on_one_side() -> None:
    cases = many(60, phrasings=3)
    split = split_cases(cases, key=lambda case: case.question, salt="run-1")

    dev = {case.question for case in split.dev}
    test = {case.question for case in split.test}
    assert not dev & test


def test_adding_a_question_never_moves_an_existing_one() -> None:
    before = split_cases(many(40), key=lambda case: case.question, salt="run-1")
    after = split_cases(many(60), key=lambda case: case.question, salt="run-1")

    held_out_before = {case.question for case in before.test}
    held_out_after = {case.question for case in after.test}
    original = {case.question for case in many(40)}
    assert held_out_before == held_out_after & original


def test_a_different_salt_may_deal_a_question_the_other_way() -> None:
    cases = many(60)
    one = {case.question for case in split_cases(cases, key=lambda c: c.question, salt="a").test}
    other = {case.question for case in split_cases(cases, key=lambda c: c.question, salt="b").test}
    assert one != other


def test_the_share_is_roughly_what_was_asked_for() -> None:
    cases = many(1_000)
    split = split_cases(cases, key=lambda case: case.question, salt="run-1", held_out=30)
    assert 25 <= len(split.test) / len(cases) * 100 <= 35


def test_an_empty_held_out_side_is_refused_rather_than_recorded() -> None:
    # Two questions at the default share: the deal may put both on dev.
    with pytest.raises(RegistryError) as refusal:
        split_cases(many(120), key=lambda case: "one-question", salt="run-1")

    assert "held-out side is empty" in str(refusal.value)
    assert "1 group(s) across 120 case(s)" in str(refusal.value)


def test_the_refusal_counts_groups_rather_than_cases() -> None:
    # Six phrasings of one question is one question, and the message has to say
    # so — "add questions" is the fix, and "120 cases" would hide it.
    with pytest.raises(RegistryError) as refusal:
        split_cases(many(1, phrasings=6), key=lambda case: case.question, salt="run-1")

    assert "1 group(s) across 6 case(s)" in str(refusal.value)


def test_a_share_outside_one_to_ninety_nine_is_refused() -> None:
    for share in (0, 100, -10):
        with pytest.raises(RegistryError):
            held_out_for("q1", "run-1", share)


def test_the_split_carries_the_group_counts_it_could_not_recompute() -> None:
    split = split_cases(many(60, phrasings=2), key=lambda case: case.question, salt="run-1")

    assert split.dev_groups + split.test_groups == 60
    assert len(split.dev) == split.dev_groups * 2
    assert len(split.test) == split.test_groups * 2


def test_order_within_a_side_is_the_order_it_was_given() -> None:
    cases = many(60)
    split = split_cases(cases, key=lambda case: case.question, salt="run-1")

    assert list(split.dev) == [case for case in cases if case in set(split.dev)]


def test_the_default_share_is_the_one_the_module_documents() -> None:
    cases = many(200)
    named = split_cases(cases, key=lambda c: c.question, salt="s", held_out=HELD_OUT_PERCENT)
    default = split_cases(cases, key=lambda c: c.question, salt="s")
    assert named == default
