"""The typed SDK vocabulary follows the Rust-owned wire contract."""

import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from typing import Any, Literal, NotRequired, get_args, get_origin, get_type_hints

import jsonschema
import pytest

from aiwatcher_sdk import evaluation

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = ROOT / "contracts/fixtures/evaluation-v1"
SCHEMA = json.loads((ROOT / "contracts/evaluation-manifest.schema.json").read_text())


def test_the_shared_example_conforms_to_the_generated_wire_schema() -> None:
    jsonschema.Draft202012Validator.check_schema(SCHEMA)
    jsonschema.validate(json.loads((FIXTURES / "manifest.json").read_text()), SCHEMA)


@pytest.mark.parametrize("name", evaluation.__all__)
def test_the_python_contract_keeps_the_rust_fields_and_required_metadata(name: str) -> None:
    definition = SCHEMA["$defs"]["ArtifactRef" if name == "ArtifactReference" else name]
    cls = getattr(evaluation, name)
    fields = get_type_hints(cls, include_extras=True)
    assert set(fields) == set(definition["properties"])
    required = set(definition["required"])
    # Core permits a missing size; the Evaluation facade and its producer
    # types deliberately require it without changing other artifact readers.
    if name == "ArtifactReference":
        required.add("size_bytes")
    assert cls.__required_keys__ == required
    for field, annotation in fields.items():
        if get_origin(annotation) is NotRequired:
            annotation = get_args(annotation)[0]
        if get_origin(annotation) is Literal:
            prop = definition["properties"][field]
            if "$ref" in prop:
                prop = SCHEMA["$defs"][prop["$ref"].split("/")[-1]]
            if "enum" in prop:
                assert set(get_args(annotation)) == set(prop["enum"])
            else:
                assert get_args(annotation) == (prop["minimum"],) == (prop["maximum"],)


def test_fixture_artifact_digests_and_lengths_name_the_actual_bytes() -> None:
    def check(value: Any) -> None:
        if isinstance(value, dict):
            if "uri" in value:
                data = (FIXTURES / value["name"]).read_bytes()
                assert value["digest"] == hashlib.sha256(data).hexdigest()
                assert value["size_bytes"] == len(data)
            for child in value.values():
                check(child)
        elif isinstance(value, list):
            for child in value:
                check(child)

    check(json.loads((FIXTURES / "manifest.json").read_text()))


def test_the_example_scorer_does_not_normalize_or_ignore_empty_answers() -> None:
    spec = importlib.util.spec_from_file_location("fixture_scorer", FIXTURES / "scorer.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert module.score("Warsaw", "Warsaw") == 1.0
    assert module.score("warsaw", "Warsaw") == 0.0
    assert module.score("Warsaw ", "Warsaw") == 0.0
    assert module.score("", "") == 1.0
    assert module.score(" ", "") == 0.0


def test_evaluation_contracts_do_not_import_a_worker_transport_or_model_stack() -> None:
    subprocess.run(
        [
            sys.executable,
            "-c",
            """
import sys
import aiwatcher_sdk
assert 'aiwatcher_sdk.evaluation' not in sys.modules
import aiwatcher_sdk.evaluation
for name in ('httpx', 'aiwatcher_sdk.worker', 'aiwatcher_sdk.api', 'deepeval', 'torch'):
    assert name not in sys.modules, name
""",
        ],
        check=True,
    )
