"""Which cases an optimiser may search on, and which decide.

:meth:`~aiwatcher_sdk.prompts.PromptRegistry.record_optimization` takes ``dev``
and ``test`` and admits a candidate on the second only, because an optimiser
selected its candidate by maximising the first. The DeepEval bridge in this
package says the split is the caller's job and then leaves them to it — which
is how every caller writes the same three lines slightly differently, and how
one of them ends up passing the search scores twice.

This is that split, written once.

## The group is the unit, not the case

The same rule the annotation corpus keeps, arriving from the other side. There
a ``group_id`` is the building, and splitting one building's four drawings
apart makes the test score a measurement of memorisation with nothing in the
numbers to say so. Here a group is the **question**: two paraphrases of one
request, a multi-turn flow's several goldens, a scenario and its prefill. Deal
those apart and the optimiser tunes on one phrasing of a question whose other
phrasing then scores it.

So ``key`` is a parameter with no default worth guessing. Passing
``lambda case: case.id`` — one group per case — is a legitimate choice for a
corpus of unrelated one-shot questions, and it should be a choice somebody made
rather than one they inherited.

## An empty side is refused

`app/training/run.py`'s rule, for a different metric: a score computed over
nothing is worse than a missing one, because it looks like a score. An
optimisation whose held-out side is empty reports a number the registry would
then admit a promotion on, and the number is about no cases at all. Both sides
are checked, and the refusal says which is empty and how many *groups* there
were — because with three groups at thirty percent the answer is that there are
not enough questions yet, not that the share is wrong.
"""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Final

from aiwatcher_sdk.prompts import RegistryError

__all__ = ["HELD_OUT_PERCENT", "CaseSplit", "held_out_for", "split_cases"]

#: What :func:`split_cases` holds out when the caller names no share.
#:
#: Thirty rather than the annotation corpus's fifteen: an optimisation's whole
#: evidence is one number on one metric, and fifteen percent of a few dozen
#: questions is a held-out side where one case decides the verdict.
HELD_OUT_PERCENT: Final = 30


@dataclass(frozen=True, slots=True)
class CaseSplit[Case]:
    """The two sides, named for the arguments they are about to become.

    ``dev`` and ``test``, spelled exactly as ``record_optimization`` spells
    them, so a call site reads ``dev=split.dev, test=split.test`` and a swapped
    pair is visible rather than plausible.

    The group counts are carried rather than derived, because the keys are gone
    by the time anybody holds this: a caller reporting "searched 41 cases over
    12 questions, decided on 9 over 4" cannot recompute that from two tuples.
    """

    dev: tuple[Case, ...]
    test: tuple[Case, ...]
    dev_groups: int
    test_groups: int


def held_out_for(group: str, salt: str, held_out: int = HELD_OUT_PERCENT) -> bool:
    """Whether one group is held out.

    Deterministic in the group and the salt and *only* in those, which is what
    makes adding a question leave every existing question where it was. The
    same shape as the annotation corpus's ``split_for``, and deliberately not
    the same function: that one answers three sides for fitting a model, this
    one answers two for deciding an optimisation, and a caller reaching for the
    first would have to pick which of ``train`` and ``validation`` means
    ``dev``.
    """
    if not 0 < held_out < 100:
        raise RegistryError(f"held-out share must be between 1 and 99 percent, got {held_out}")
    digest = hashlib.sha256(salt.encode() + b"\x00" + group.encode()).digest()
    return int.from_bytes(digest[:8], "big") % 100 >= 100 - held_out


def split_cases[Case](
    cases: Sequence[Case],
    *,
    key: Callable[[Case], str],
    salt: str,
    held_out: int = HELD_OUT_PERCENT,
) -> CaseSplit[Case]:
    """Deal cases into a search side and a deciding side, by group.

    ``key`` names the question a case is one phrasing of; every case sharing a
    key lands on one side. ``salt`` fixes the deal — change it and every group
    may move, which is a thing to do deliberately and never between measuring
    the baseline and measuring the candidate.

    Order is preserved within each side, so a run is reproducible without the
    caller sorting anything.
    """
    dev: list[Case] = []
    test: list[Case] = []
    dev_groups: set[str] = set()
    test_groups: set[str] = set()
    for case in cases:
        group = key(case)
        if held_out_for(group, salt, held_out):
            test.append(case)
            test_groups.add(group)
        else:
            dev.append(case)
            dev_groups.add(group)
    if not dev or not test:
        empty = "dev" if not dev else "held-out"
        groups = len(dev_groups | test_groups)
        raise RegistryError(
            f"the {empty} side is empty: {groups} group(s) across {len(cases)} case(s) at "
            f"{held_out}% held out. A score over no cases still looks like a score, so this "
            f"is refused rather than recorded — add questions, or state a different share."
        )
    return CaseSplit(
        dev=tuple(dev),
        test=tuple(test),
        dev_groups=len(dev_groups),
        test_groups=len(test_groups),
    )
