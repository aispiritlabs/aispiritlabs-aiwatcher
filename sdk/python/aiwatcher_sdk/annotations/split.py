"""Which side of a corpus a group falls on, and the three names for the sides.

The *rule*, not the view over its result — :class:`~aiwatcher_sdk.annotations.view.SplitView`
is that, and it is a file further up. They are apart because a sample has to
name its side, so the vocabulary that names the sides cannot depend on the
sample that carries one.
"""

from __future__ import annotations

import hashlib
from typing import Literal, get_args

from aiwatcher_sdk.annotations.errors import RegistryError

__all__ = ["SPLITS", "Split", "split_for"]

Split = Literal["train", "validation", "test"]

#: The three sides, in the order a report reads them.
SPLITS: tuple[Split, ...] = get_args(Split)


def split_for(group_id: str, salt: str, ratios: tuple[int, int, int] = (70, 15, 15)) -> Split:
    """Which side of the split a group falls on.

    The same computation the server does, byte for byte, so a caller can answer
    "is this house in the test set" without a request. Deterministic in the
    group and the salt and *only* in those: adding an image never moves an
    existing group.
    """
    if sum(ratios) != 100 or any(ratio < 0 for ratio in ratios):
        raise RegistryError(f"split ratios must be three non-negative parts of 100, got {ratios}")
    digest = hashlib.sha256(salt.encode() + b"\x00" + group_id.encode()).digest()
    bucket = int.from_bytes(digest[:8], "big") % 100
    train, validation, _ = ratios
    if bucket < train:
        return "train"
    if bucket < train + validation:
        return "validation"
    return "test"
