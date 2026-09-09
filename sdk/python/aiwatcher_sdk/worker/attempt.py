"""Operations available to a single leased attempt."""

from collections.abc import Sequence
from typing import Protocol

from aiwatcher_sdk.worker.contract import ArtifactRef, JsonObject, Report


class AttemptAPI(Protocol):
    def heartbeat(self) -> None: ...

    def read_artifact(self, name: str) -> list[JsonObject]: ...

    def write_artifact(self, name: str, rows: Sequence[JsonObject]) -> ArtifactRef: ...

    def report(self, report: Report) -> None: ...
