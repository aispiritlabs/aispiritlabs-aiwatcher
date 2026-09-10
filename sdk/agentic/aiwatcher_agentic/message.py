"""Conversation turns and how they render into a prompt.

The turn markers below are Gemma-style. Gemma has no separate ``system`` role —
the system prompt is rendered as a ``model`` turn — which is why both
:class:`SystemMessage` and :class:`AssistantMessage` carry ``role = "model"``.
Builders for other families (see ``aiwatcher_agentic.prompts.QwenPromptBuilder``) detect
and pass through their own markers.
"""

from __future__ import annotations

from base64 import b64encode
from dataclasses import dataclass
from typing import Any, ClassVar

__all__ = [
    "AssistantMessage",
    "ImagePart",
    "Message",
    "SystemMessage",
    "ToolMessage",
    "UserMessage",
]

TURN_START = "<start_of_turn>"
TURN_END = "<end_of_turn>"
TOOL_RESULT_START = "<tool_call_response>"
TOOL_RESULT_END = "</tool_call_response>"


class Message:
    """A single conversation turn."""

    role: ClassVar[str] = ""

    def __init__(
        self,
        content: str | None = None,
        structural_message: dict[str, Any] | None = None,
    ):
        self.content = content
        self.structural_message = structural_message

    def get_text(self) -> str:
        if self.content is not None:
            return self.content
        if isinstance(self.structural_message, dict):
            content = self.structural_message.get("content")
            if isinstance(content, str):
                return content
        return ""

    def as_turn(self) -> str:
        """Render this message as a prompt turn."""
        return f"{TURN_START}{self.role}\n{self.get_text()}\n{TURN_END}"

    def __str__(self) -> str:
        return self.get_text()

    def __repr__(self) -> str:
        return f"{type(self).__name__}(role={self.role!r}, content={self.get_text()!r})"


class AssistantMessage(Message):
    """A reply produced by the model."""

    role: ClassVar[str] = "model"


class SystemMessage(Message):
    """Instructions given to the model.

    Rendered as a ``model`` turn because the Gemma chat template has no
    dedicated system role.
    """

    role: ClassVar[str] = "model"


class ToolMessage(Message):
    """The result of a tool call, fed back to the model."""

    role: ClassVar[str] = "tool"

    def as_turn(self) -> str:
        return f"{TOOL_RESULT_START}{self.get_text()}\n{TOOL_RESULT_END}"


class UserMessage(Message):
    """A turn written by the user."""

    role: ClassVar[str] = "user"


@dataclass(frozen=True, slots=True)
class ImagePart:
    """An image to send with a turn.

    Held as bytes rather than a path because the two things that produce one —
    a rendered PDF page and a screenshot — never touch disk, and because the
    wire form is a data URI either way. `identifier` is for traces and error
    messages; it never reaches the model.
    """

    data: bytes
    media_type: str = "image/png"
    identifier: str = ""

    def as_content_part(self) -> dict[str, Any]:
        encoded = b64encode(self.data).decode("ascii")
        return {
            "type": "image_url",
            "image_url": {"url": f"data:{self.media_type};base64,{encoded}"},
        }

    def __repr__(self) -> str:
        name = self.identifier or "<unnamed>"
        return f"ImagePart({name!r}, {self.media_type}, {len(self.data)} bytes)"
