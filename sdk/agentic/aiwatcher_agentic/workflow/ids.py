"""Ids the engine mints itself: time-ordered, so a list of them sorts by age."""

from __future__ import annotations

import os
import sys
import time
import uuid

if sys.version_info >= (3, 14):
    from uuid import uuid7
else:

    def uuid7() -> uuid.UUID:
        """RFC 9562 version 7: 48 bits of Unix milliseconds, then random bits.

        The standard library has it from 3.14 and this distribution's floor is
        3.13 — the engine called ``uuid.uuid7`` when it only ran on 3.14, which
        on 3.13 raises the first time a runtime mints an id. Ordered to the
        millisecond; inside one millisecond the standard library's counter keeps
        its ids in order and this does not.
        """
        milliseconds = time.time_ns() // 1_000_000
        value = (milliseconds & ((1 << 48) - 1)) << 80 | int.from_bytes(os.urandom(10))
        value = (value & ~(0xF << 76)) | (0x7 << 76)
        value = (value & ~(0x3 << 62)) | (0x2 << 62)
        return uuid.UUID(int=value)


__all__ = ["uuid7"]
