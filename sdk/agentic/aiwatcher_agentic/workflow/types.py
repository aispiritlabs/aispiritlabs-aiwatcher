"""Nominal types for key identifiers in the event sourcing system.

Use these types in function signatures to prevent accidentally mixing up
identifiers that are all plain strings at runtime. The type checker will
catch e.g. passing a TurnId where a SessionId is expected.
"""

from __future__ import annotations

from typing import NewType

# --- Identifiers ---
EventId = NewType("EventId", str)
MessageId = NewType("MessageId", str)
SessionId = NewType("SessionId", str)
RuntimeId = NewType("RuntimeId", str)
TurnId = NewType("TurnId", str)
StreamName = NewType("StreamName", str)

# --- Positions ---
StreamPosition = NewType("StreamPosition", int)
GlobalPosition = NewType("GlobalPosition", int)
