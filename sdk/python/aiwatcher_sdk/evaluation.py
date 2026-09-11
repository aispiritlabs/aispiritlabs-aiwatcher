"""Wire contracts for pinned evaluations (ADR_0030).

These are TypedDicts, not a runtime validator or a registry client. The
Evaluation facade validates semantics and computes IDs; callers must not infer
durability from constructing a manifest. Importing this module needs only the
standard library and leaves the telemetry client's failure policy unchanged.
"""

from typing import Literal, NotRequired, TypedDict

__all__ = [
    "ArtifactReference",
    "DatasetReference",
    "EvaluationContext",
    "EvaluationManifest",
    "EvaluationOrigin",
    "JudgeConfiguration",
    "MetricDefinition",
    "VariantManifest",
    "VersionReference",
]


class ArtifactReference(TypedDict):
    name: str
    uri: str
    digest: str
    size_bytes: int
    content_type: NotRequired[str]
    kind: NotRequired[Literal["rows", "code", "preview", "log", "model", "report", "blob"]]
    schema_ref: NotRequired[str | None]


class VersionReference(TypedDict):
    name: str
    version: str


class DatasetReference(TypedDict):
    kind: Literal["curation", "annotations", "conversations", "external"]
    name: str
    version: str


class MetricDefinition(TypedDict):
    name: str
    unit: str
    direction: Literal["higher", "lower", "none"]
    aggregation: Literal["mean", "sum", "min", "max", "rate", "none"]


class JudgeConfiguration(TypedDict):
    provider: str
    model: VersionReference
    configuration: ArtifactReference
    calibration_dataset: DatasetReference


class EvaluationContext(TypedDict):
    dataset: DatasetReference
    case_manifest: ArtifactReference
    case_count: int
    split: str
    suite: VersionReference
    scorer: VersionReference
    input_schema: ArtifactReference
    expectations_schema: ArtifactReference
    judge: NotRequired[JudgeConfiguration | None]
    metrics: list[MetricDefinition]


class VariantManifest(TypedDict):
    schema_version: Literal[1]
    experiment_id: str
    dataset: DatasetReference
    model: NotRequired[VersionReference | None]
    prompt: NotRequired[VersionReference | None]
    code: ArtifactReference
    generation_config: ArtifactReference
    response_schema: NotRequired[ArtifactReference | None]
    tools: NotRequired[ArtifactReference | None]
    workflow: NotRequired[VersionReference | None]


class EvaluationOrigin(TypedDict):
    evaluation_id: str
    repetition_id: str
    execution_id: NotRequired[str | None]
    step_id: NotRequired[str | None]


class EvaluationManifest(TypedDict):
    schema_version: Literal[1]
    origin: EvaluationOrigin
    variant: VariantManifest
    context: EvaluationContext
