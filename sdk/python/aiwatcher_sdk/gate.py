"""``aiwatcher-gate``: measure a variant in CI and exit by the server's verdict.

A pipeline has one question to ask aiwatcher and one bit to act on, and the
answer is the server's: the same comparison the panel draws, held to a policy.
This is the recipe around it — stage what the variant pins, declare and start
the measurement, follow the run, ask the gate, say why, exit::

    aiwatcher-gate --url "$AIWATCHER_URL" --run run.json --baseline answers-main \\
        --policy policy.json --code-commit --recording answers.json

Exit codes, so a job can tell a change somebody made from a measurement that
could not say:

- ``0`` pass;
- ``1`` regression — a metric worse than its tolerance, or a critical case lost;
- ``2`` incomplete — a scorer failed or a case went unanswered, which never passes;
- ``3`` error — nothing admits the pair, the run failed, or the two do not compare.

A variant naming a model or a workflow is admitted through a line too: the
server stages a registered model's package from its own registry, and the job
sends the bytes nobody there holds — the model's weights, the workflow's
declaration, and the ``model-package.json`` of a model the server never
registered, whose digest is that variant's model version — with ``--stage``,
addressed by their digest.

It records what a reader needs later beside the verdict: the commit, the card
version the context pins, the variant's ID — what the deployed application
names on its runs, so what it is observed doing stands beside what it scored —
and the link to the evidence. With
``GITHUB_STEP_SUMMARY`` set it writes the same as Markdown there. It comments on
no pull request. A regression suite a team can see is not a held-out measure of
improvement, and passing it is not one.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import quote

from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.evaluation_registry import EvaluationRegistry

EXIT = {"pass": 0, "regression": 1, "incomplete": 2, "error": 3}
TERMINAL = frozenset({"completed", "failed", "cancelled", "crashed"})
PINNED = ("code", "generation_config", "response_schema", "tools")


@dataclass(frozen=True)
class Outcome:
    """What the gate decided, and what a pipeline records beside it."""

    verdict: str
    reasons: list[str]
    evaluation_id: str
    evidence: str
    commit: str | None
    decision: dict[str, Any] | None
    variant_id: str | None = None

    @property
    def exit_code(self) -> int:
        return EXIT[self.verdict]


def commit_note(repository: str | None, commit: str) -> bytes:
    """A variant's code as the commit that holds it, in bytes a pin can name."""
    return json.dumps(
        {"repository": repository, "commit": commit}, sort_keys=True, separators=(",", ":")
    ).encode()


def run_gate(
    registry: EvaluationRegistry,
    run: dict[str, Any],
    *,
    baseline: str,
    policy: Mapping[str, Any],
    artifacts: Mapping[str, Path],
    recording: Path | None,
    staged: Sequence[Path] = (),
    commit: str | None,
    repository: str | None,
    code_commit: bool,
    panel_url: str,
    timeout: float,
    sleep: Callable[[float], None] = time.sleep,
    clock: Callable[[], float] = time.monotonic,
) -> Outcome:
    """Stage, declare, start, follow and gate one variant's measurement."""
    variant = run["variant"]
    evaluation_id = str(run["evaluation_id"])
    evidence = f"{panel_url.rstrip('/')}/evaluation?evidence={quote(evaluation_id, safe='')}"

    def failed(reason: str) -> Outcome:
        return Outcome("error", [reason], evaluation_id, evidence, commit, None)

    if code_commit:
        if commit is None:
            return failed("--code-commit needs a commit: --commit or GITHUB_SHA")
        variant["code"] = registry.stage_variant_artifact(
            "commit.json", commit_note(repository, commit)
        )
    for field, path in artifacts.items():
        variant[field] = registry.stage_variant_artifact(path.name, path.read_bytes())
    for path in staged:
        registry.stage_variant_artifact(path.name, path.read_bytes())
    if recording is not None:
        answers = json.loads(recording.read_bytes())
        run["answers"] = registry.stage_recording(recording.name, answers["answers"])
    # A policy holding results to a lookback declares the run with at least as
    # much, so the run it starts reads what the gate will ask it to have read.
    if (lookback := policy.get("asked_since_seconds")) is not None:
        settings = run.setdefault("settings", {})
        settings["asked_since_seconds"] = max(
            int(settings.get("asked_since_seconds") or 0), int(lookback)
        )

    try:
        view = registry.declare_scoring_run(run)  # type: ignore[arg-type]
        started = registry.start_scoring_run(view["declaration"]["id"])
    except ApiError as error:
        if error.code == "pair_not_admitted":
            return failed(
                f"nothing admits this variant yet ({error}); an admin admits the pair, or once "
                "for every variant of this experiment in this context with "
                "POST /api/v1/evaluation-approval-lines"
            )
        raise
    variant_id = str(view.get("variant_id")) if view.get("variant_id") else None
    execution_id = started["execution"]["execution_id"]
    deadline = clock() + timeout
    while True:
        state = registry.get_execution(execution_id)["execution"]["state"]
        if state["state_type"] in TERMINAL:
            break
        if clock() > deadline:
            return failed(f"run {execution_id} did not finish within {timeout:.0f}s")
        sleep(1.0)
    if state["state_type"] != "completed":
        return Outcome(
            "error",
            [
                f"run {execution_id} ended {state['state_type']}"
                + (f": {state['message']}" if state.get("message") else "")
            ],
            evaluation_id,
            evidence,
            commit,
            None,
            variant_id,
        )
    decision = registry.gate(evaluation_id, baseline=baseline, policy=policy)
    return Outcome(
        str(decision["verdict"]),
        list(decision["reasons"]),
        evaluation_id,
        evidence,
        commit,
        decision,
        variant_id,
    )


def summary(outcome: Outcome) -> str:
    """The verdict in Markdown, as a job summary shows it."""
    lines = [f"## aiwatcher gate: {outcome.verdict}", ""]
    decision = outcome.decision or {}
    candidate = decision.get("candidate") or {}
    suite = candidate.get("suite") or {}
    lines += [
        f"- evidence: [{outcome.evaluation_id}]({outcome.evidence})",
        f"- commit: `{outcome.commit}`" if outcome.commit else "- commit: not given",
    ]
    if outcome.variant_id:
        lines.append(f"- variant: `{outcome.variant_id}`")
    if suite:
        lines.append(f"- card: `{suite.get('name')}` @ `{str(suite.get('version'))[:12]}`")
    if decision.get("baseline"):
        lines.append(f"- baseline: `{decision['baseline']['evaluation_id']}`")
    metrics = decision.get("metrics") or []
    if metrics:
        lines += ["", "| metric | candidate | baseline | change | held |", "|---|---|---|---|---|"]
        for metric in metrics:
            lines.append(
                f"| {metric['name']} | {metric.get('current', '—')} | "
                f"{metric.get('baseline', '—')} | {metric.get('delta', 'withheld')} | "
                f"{'regressed' if metric['regressed'] else ('yes' if metric['held'] else 'no')} |"
            )
    if outcome.reasons:
        lines += ["", *[f"- {reason}" for reason in outcome.reasons]]
    return "\n".join(lines) + "\n"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="aiwatcher-gate", description=__doc__.split("\n")[0])
    parser.add_argument("--url", default=os.environ.get("AIWATCHER_URL", "http://localhost:8080"))
    parser.add_argument("--token", default=os.environ.get("AIWATCHER_TOKEN"))
    parser.add_argument("--panel-url", help="where the evidence link points; --url by default")
    parser.add_argument("--run", type=Path, required=True, help="a scoring run declaration")
    parser.add_argument("--baseline", required=True, help="the result to hold the candidate to")
    parser.add_argument("--policy", type=Path, help="tolerance, ignore and critical_cases")
    parser.add_argument(
        "--artifact",
        action="append",
        default=[],
        metavar="FIELD=PATH",
        help=f"stage a file the variant pins as one of {', '.join(PINNED)}; repeatable",
    )
    parser.add_argument("--recording", type=Path, help="recorded answers to stage and score")
    parser.add_argument(
        "--stage",
        action="append",
        default=[],
        type=Path,
        metavar="PATH",
        help="send bytes a line needs by digest — a model's weights, a workflow's declaration; "
        "repeatable",
    )
    parser.add_argument("--evaluation-id", help="the result's ID, e.g. answers-$GITHUB_SHA")
    parser.add_argument("--commit", default=os.environ.get("GITHUB_SHA"))
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument(
        "--code-commit",
        action="store_true",
        help="pin variant.code to the commit and repository, staged as commit.json",
    )
    parser.add_argument("--timeout", type=float, default=900.0)
    parser.add_argument("--output", type=Path, help="write the decision as JSON here")
    args = parser.parse_args(argv)

    artifacts: dict[str, Path] = {}
    for entry in args.artifact:
        field, separator, path = entry.partition("=")
        if not separator or field not in PINNED:
            parser.error(f"--artifact takes FIELD=PATH with FIELD one of {', '.join(PINNED)}")
        artifacts[field] = Path(path)
    run = json.loads(args.run.read_text())
    if args.evaluation_id:
        run["evaluation_id"] = args.evaluation_id
    policy = json.loads(args.policy.read_text()) if args.policy else {}

    with EvaluationRegistry(args.url, token=args.token) as registry:
        try:
            outcome = run_gate(
                registry,
                run,
                baseline=args.baseline,
                policy=policy,
                artifacts=artifacts,
                recording=args.recording,
                staged=args.stage,
                commit=args.commit,
                repository=args.repository,
                code_commit=args.code_commit,
                panel_url=args.panel_url or args.url,
                timeout=args.timeout,
            )
        except ApiError as error:
            outcome = Outcome(
                "error", [str(error)], str(run.get("evaluation_id")), "", args.commit, None
            )

    text = summary(outcome)
    sys.stdout.write(text)
    if step_summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(step_summary).open("a") as handle:
            handle.write(text)
    if args.output:
        args.output.write_text(
            json.dumps(
                {
                    "verdict": outcome.verdict,
                    "reasons": outcome.reasons,
                    "evaluation_id": outcome.evaluation_id,
                    "evidence": outcome.evidence,
                    "commit": outcome.commit,
                    "variant_id": outcome.variant_id,
                    "decision": outcome.decision,
                },
                indent=2,
            )
        )
    return outcome.exit_code


if __name__ == "__main__":
    raise SystemExit(main())
