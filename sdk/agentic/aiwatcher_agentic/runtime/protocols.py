from __future__ import annotations

from typing import Protocol


class RuntimeProtocol(Protocol):
    """Structural protocol for agent orchestration runtimes.

    Defines the minimal interface that any runtime must satisfy.
    Application-specific methods (e.g. run_chat, run_generate_image)
    belong in extended protocols defined by the application.
    """

    @property
    def runtime_id(self) -> str: ...

    def run(self, text: str) -> str: ...

    def handle(self, message: object) -> str: ...

    def start(self) -> str: ...

    def stop(self) -> None: ...
