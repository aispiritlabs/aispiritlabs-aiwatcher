"""Wire contracts for pinned evaluations (ADR_0030).

These are TypedDicts, not a runtime validator or a registry client. The
Evaluation facade validates semantics and computes IDs; callers must not infer
durability from constructing a manifest. Importing this module needs only the
standard library and leaves the telemetry client's failure policy unchanged.
"""

from typing import Literal, NotRequired, TypedDict

__all__ = [
    "ArtifactReference",
    "CalibrationPin",
    "DatasetReference",
    "EvaluationContext",
    "EvaluationManifest",
    "EvaluationOrigin",
    "ExternalMeasure",
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
    kind: Literal["curation", "annotations", "conversations", "external", "assessments"]
    name: str
    version: str


class ExternalMeasure(TypedDict):
    """A metric a scorer framework measured, through the deployment's scorer service."""

    adapter: VersionReference
    metric: str
    #: The model that graded it; its numbers are not reproduced by re-reading.
    model: NotRequired[VersionReference | None]


class MetricDefinition(TypedDict):
    name: str
    unit: str
    direction: Literal["higher", "lower", "none"]
    aggregation: Literal["mean", "sum", "min", "max", "rate", "none"]
    #: Derived by the server for a framework's metric; absent otherwise.
    measured_by: NotRequired[ExternalMeasure | None]


class JudgeConfiguration(TypedDict):
    provider: str
    model: VersionReference
    configuration: ArtifactReference
    calibration_dataset: DatasetReference
    #: The judge is sent words from the conversation archive. Derived by the
    #: server; absent when false.
    reads_archive: NotRequired[bool]


class CalibrationPin(TypedDict):
    """The calibration set a context's framework metrics were held against."""

    calibration_dataset: DatasetReference
    #: The set's answers come from the conversation archive. Derived by the
    #: server; absent when false.
    reads_archive: NotRequired[bool]


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
    external_calibration: NotRequired[CalibrationPin | None]
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
