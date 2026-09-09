"""Local execution capacity. Cluster replicas require an infrastructure controller."""

from dataclasses import dataclass


@dataclass(frozen=True)
class ExecutionPool:
    name: str
    queue: str
    concurrency: int = 1

    def __post_init__(self) -> None:
        if not self.name.strip() or not self.queue.strip():
            raise ValueError("an execution pool needs a name and queue")
        if self.concurrency < 0:
            raise ValueError("pool concurrency cannot be negative")


@dataclass(frozen=True)
class PoolStatus:
    name: str
    queue: str
    desired: int
    running: int
    draining: int
