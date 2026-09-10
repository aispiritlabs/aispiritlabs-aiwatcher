import re
from collections.abc import Callable
from pathlib import Path
from typing import Any, Final, Protocol

import orjson

from aiwatcher_agentic.message import Message, SystemMessage, UserMessage
from aiwatcher_agentic.tools import Toolset, Toolsets

_PLACEHOLDER: Final = re.compile(r"{{\s*([a-zA-Z][a-zA-Z0-9_]*)\s*}}")
_PROMPT_NAME: Final = re.compile(r"[a-z0-9][a-z0-9_./-]*\.md")


class MarkdownPromptBuilder:
    """Render a Markdown template, replacing its declared ``{{ variables }}``.

    Not the same job as :class:`PromptBuilder` below, which assembles the turns
    of a chat prompt. This one loads prompt *text* from files that a human edits
    and a reviewer reads as a diff, which is what a prompt of any size wants to
    be.

    The strictness is the point. A missing value and an unexpected one are both
    errors, so renaming a placeholder cannot silently produce a prompt with a
    literal ``{{ hint }}`` in it, and deleting one cannot leave a caller quietly
    passing a value nobody reads. Single braces are untouched, so a template may
    contain ``{tools}`` for the layer above to fill in.
    """

    def __init__(self, directory: Path) -> None:
        self._directory = directory.resolve()
        self._cache: dict[str, str] = {}

    def _template(self, name: str) -> str:
        if not _PROMPT_NAME.fullmatch(name) or ".." in Path(name).parts:
            raise ValueError(f"Invalid prompt template name: {name!r}.")
        if cached := self._cache.get(name):
            return cached
        path = (self._directory / name).resolve()
        if not path.is_relative_to(self._directory):
            raise ValueError(f"Prompt template is outside the prompt directory: {name!r}.")
        try:
            template = path.read_text(encoding="utf-8")
        except FileNotFoundError as error:
            raise ValueError(f"Prompt template not found: {name!r}.") from error
        if not template.strip():
            raise ValueError(f"Prompt template is empty: {name!r}.")
        template = template.strip()
        self._cache[name] = template
        return template

    def build(self, name: str, /, **values: object) -> str:
        template = self._template(name)
        placeholders = set(_PLACEHOLDER.findall(template))
        supplied = set(values)
        if missing := placeholders - supplied:
            raise ValueError(f"Missing prompt values for {name!r}: {', '.join(sorted(missing))}.")
        if unexpected := supplied - placeholders:
            raise ValueError(
                f"Unexpected prompt values for {name!r}: {', '.join(sorted(unexpected))}."
            )
        return _PLACEHOLDER.sub(lambda match: str(values[match.group(1)]), template)


#: Where a prompt named by ``external_prompt_name`` is read from: a name in,
#: the prompt's text out.
type PromptSource = Callable[[str], str]

_prompt_source: PromptSource | None = None


def use_prompt_source(source: PromptSource | None) -> None:
    """Name where a builder given ``external_prompt_name`` and no source reads.

    The application's call, made once where it starts. A builder used to import
    the MLflow prompt registry itself, which is an application package; the
    application names it here instead — `ai_spirit_agent`'s `agentic` does, on
    import — and the same hook takes ADR_0011's registry when that replaces it.
    ``None`` forgets the source.
    """
    global _prompt_source  # one process default, set where the application starts
    _prompt_source = source


def read_external_prompt(name: str, source: PromptSource | None = None) -> str:
    """The text of the prompt ``name``, from ``source`` or the process default."""
    resolved = source if source is not None else _prompt_source
    if resolved is None:
        raise LookupError(
            f"Prompt {name!r} is named by external_prompt_name and there is no prompt "
            "source to read it from: pass prompt_source=, or call use_prompt_source() "
            "where the application starts."
        )
    return resolved(name)


class PromptBuilder(Protocol):
    @property
    def system_prompt(self) -> str: ...
    @system_prompt.setter
    def system_prompt(self, value: str) -> None: ...

    @property
    def external_prompt_name(self) -> str | None: ...
    @external_prompt_name.setter
    def external_prompt_name(self, value: str | None) -> None: ...

    def build_prompt(
        self,
        message: str | Message,
        system_prompt: str = "",
        toolsets: Toolsets | None = None,
    ) -> str | list[dict[str, str]]: ...

    def tools_instruction(self, toolset: Toolset) -> str: ...


class CorePromptBuilder:
    """Base class for prompt builders with shared functionality."""

    def __init__(
        self,
        *,
        system_prompt: str | None = None,
        external_prompt_name: str | None = None,
        prompt_source: PromptSource | None = None,
    ) -> None:
        if system_prompt is not None:
            self._system_prompt = system_prompt
        elif external_prompt_name is not None:
            self._system_prompt = read_external_prompt(external_prompt_name, prompt_source)
        else:
            self._system_prompt = ""
        self._external_prompt_name = external_prompt_name

    @property
    def system_prompt(self) -> str:
        return self._system_prompt

    @system_prompt.setter
    def system_prompt(self, value: str) -> None:
        self._system_prompt = value

    @property
    def external_prompt_name(self) -> str | None:
        return self._external_prompt_name

    @external_prompt_name.setter
    def external_prompt_name(self, value: str | None) -> None:
        self._external_prompt_name = value

    @staticmethod
    def _map_types(type_name: str) -> str:
        mapping = {
            "str": "string",
            "int": "integer",
            "float": "number",
            "bool": "boolean",
            "dict": "object",
            "list": "array",
        }
        return mapping.get(type_name, "string")

    def _is_full_turn(self, text: str) -> bool:
        """Check if text is already a full turn for this format. Override in subclasses."""
        raise NotImplementedError("Subclasses must implement _is_full_turn")

    def tools_instruction(self, toolset: Toolset) -> str:
        functions: list[dict[str, Any]] = []
        for t in toolset._tools:
            properties: dict[str, Any] = {}
            required: list[str] = []
            for arg in t.args:
                properties[arg["name"]] = {"type": self._map_types(str(arg["type"]))}
                if arg["required"]:
                    required.append(arg["name"])
            functions.append(
                {
                    "name": t.name,
                    "description": t.doc.splitlines(),
                    "parameters": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                    },
                }
            )

        lines = [
            orjson.dumps(functions, option=orjson.OPT_INDENT_2).decode("utf-8"),
        ]

        return "\n".join(lines)

    @staticmethod
    def _message_text(message: str | Message) -> str:
        if isinstance(message, Message):
            return message.get_text()
        return str(message)

    def build_prompt(
        self,
        message: str | Message,
        system_prompt: str = "",
        toolsets: Toolsets | None = None,
    ) -> str | list[dict[str, str]]:
        raise NotImplementedError


class GemmaPromptBuilder(CorePromptBuilder, PromptBuilder):
    def _is_full_turn(self, text: str) -> bool:
        candidate = text.strip()
        return candidate.startswith("<start_of_turn>") and "<end_of_turn>" in candidate

    def build_prompt(
        self,
        message: str | Message,
        system_prompt: str = "",
        toolsets: Toolsets | None = None,
    ) -> str | list[dict[str, str]]:
        message_text = self._message_text(message)
        toolset_prompt = (
            "\n".join([self.tools_instruction(toolset) for toolset in toolsets if toolset])
            if toolsets
            else ""
        )
        system_template = system_prompt or self.system_prompt
        system_text = system_template.replace("{tools}", toolset_prompt).strip()
        if self._is_full_turn(system_text):
            system_turn = system_text.strip()
        else:
            system_turn = SystemMessage(system_text).as_turn()

        if self._is_full_turn(message_text):
            user_turn = message_text.strip()
        else:
            user_turn = UserMessage(message_text).as_turn()

        return f"{system_turn}\n{user_turn}\n<start_of_turn>model\n"


class QwenPromptBuilder(CorePromptBuilder, PromptBuilder):
    def _is_full_turn(self, text: str) -> bool:
        candidate = text.strip()
        return candidate.startswith("<|im_start|>") and "<|im_end|>" in candidate

    @staticmethod
    def _is_gemma_turn(text: str) -> bool:
        candidate = text.strip()
        return candidate.startswith("<start_of_turn>") and "<end_of_turn>" in candidate

    @staticmethod
    def _wrap_turn(role: str, content: str) -> str:
        return f"<|im_start|>{role}\n{content}\n<|im_end|>"

    @classmethod
    def _gemma_to_qwen_turns(cls, text: str) -> str:
        return (
            text.replace("<start_of_turn>system\n", "<|im_start|>system\n")
            .replace("<start_of_turn>user\n", "<|im_start|>user\n")
            .replace("<start_of_turn>assistant\n", "<|im_start|>assistant\n")
            .replace("<start_of_turn>model\n", "<|im_start|>assistant\n")
            .replace("<end_of_turn>", "<|im_end|>")
        )

    def build_prompt(
        self,
        message: str | Message,
        system_prompt: str = "",
        toolsets: Toolsets | None = None,
    ) -> str | list[dict[str, str]]:
        message_text = self._message_text(message)
        toolset_prompt = (
            "\n".join([self.tools_instruction(toolset) for toolset in toolsets if toolset])
            if toolsets
            else ""
        )
        system_template = system_prompt or self.system_prompt
        system_text = system_template.replace("{tools}", toolset_prompt).strip()

        if self._is_full_turn(system_text):
            system_turn = system_text.strip()
        elif self._is_gemma_turn(system_text):
            system_turn = self._gemma_to_qwen_turns(system_text).strip()
        else:
            system_turn = self._wrap_turn("system", system_text)

        if self._is_full_turn(message_text):
            user_turn = message_text.strip()
        elif self._is_gemma_turn(message_text):
            user_turn = self._gemma_to_qwen_turns(message_text).strip()
        else:
            user_turn = self._wrap_turn("user", message_text)

        return f"{system_turn}\n{user_turn}\n<|im_start|>assistant\n"


class ChatPromptBuilder(CorePromptBuilder, PromptBuilder):
    """Prompt builder that returns OpenAI-compatible messages list."""

    def _is_full_turn(self, text: str) -> bool:
        return False

    def build_prompt(
        self,
        message: str | Message,
        system_prompt: str = "",
        toolsets: Toolsets | None = None,
    ) -> list[dict[str, str]]:
        message_text = self._message_text(message)
        toolset_prompt = (
            "\n".join([self.tools_instruction(toolset) for toolset in toolsets if toolset])
            if toolsets
            else ""
        )
        system_template = system_prompt or self.system_prompt
        system_text = system_template.replace("{tools}", toolset_prompt).strip()

        messages: list[dict[str, str]] = []
        if system_text:
            messages.append({"role": "system", "content": system_text})
        messages.append({"role": "user", "content": message_text})
        return messages


class PromptTemplate:
    def __init__(self, template: str, context_variables: list[str]):
        self._template = template
        self._context_variables = context_variables

    def format(self, **kwargs: Any) -> str:
        return self._template.format(**kwargs)
