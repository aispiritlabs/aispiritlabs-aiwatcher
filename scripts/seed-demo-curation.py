#!/usr/bin/env python3
"""Seed Flow PHP recipes, a dataset, and evaluations linked to its exact version."""

from __future__ import annotations

import argparse
import json
from datetime import UTC, datetime
from typing import Any
from urllib.error import HTTPError
from urllib.request import Request, urlopen


RECIPES = [
    {
        "name": "production/successful-conversations",
        "description": "One reproducible case per successful retained conversation.",
        "pipeline": """data_frame()
    ->read(default, period: "24h")
    ->filter(ref("status")->same(lit("succeeded")))
    ->dropDuplicates(ref("conversation_id"))
    ->rename("run_id", "source_run_id")
    ->rename("conversation_id", "source_session_id")
    ->rename("trace_id", "source_trace_id")
    ->select(
        ref("source_run_id"),
        ref("source_session_id"),
        ref("source_trace_id"),
        ref("agents"),
        ref("input_tokens"),
        ref("output_tokens"),
        ref("started_at")
    )
    ->write(to_output(truncate: false))
    ->run();""",
    },
    {
        "name": "evaluation/reviewer-and-curator",
        "description": "Runs handled by either the reviewer or curator agent.",
        "pipeline": """data_frame()
    ->read(default, period: "7d")
    ->withEntry("agent", array_expand(ref("agents")))
    ->filter(any(
        ref("agent")->same(lit("reviewer-agent")),
        ref("agent")->same(lit("curator-agent"))
    ))
    ->rename("run_id", "source_run_id")
    ->rename("conversation_id", "source_session_id")
    ->select(
        ref("source_run_id"),
        ref("source_session_id"),
        ref("agent"),
        ref("status"),
        ref("llm_calls"),
        ref("tool_calls")
    )
    ->write(to_output(truncate: false))
    ->run();""",
    },
    {
        "name": "analysis/high-token-runs",
        "description": "Successful runs with at least 2,000 input tokens.",
        "pipeline": """data_frame()
    ->read(default, period: "24h")
    ->filter(all(
        ref("status")->same(lit("succeeded")),
        ref("input_tokens")->greaterThanEqual(lit(2000))
    ))
    ->sortBy(ref("input_tokens")->desc())
    ->select(
        ref("run_id"),
        ref("conversation_id"),
        ref("agents"),
        ref("input_tokens"),
        ref("output_tokens"),
        ref("cached_tokens")
    )
    ->write(to_output(truncate: false))
    ->run();""",
    },
    {
        "name": "demo/lazy-run-explorer",
        "description": "Eighty lightweight runs for testing sliced table loading.",
        "pipeline": """data_frame()
    ->read(default, period: "24h")
    ->filter(ref("conversation_id")->same(lit("viewer-volume")))
    ->rename("run_id", "source_run_id")
    ->select(
        ref("source_run_id"),
        ref("conversation_id"),
        ref("agents"),
        ref("status"),
        ref("event_count"),
        ref("started_at")
    )
    ->write(to_output(truncate: false))
    ->run();""",
    },
]


def post(url: str, body: dict[str, Any]) -> dict[str, Any]:
    request = Request(
        url,
        data=json.dumps(body).encode(),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urlopen(request, timeout=10) as response:
            return json.load(response)
    except HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(f"{url} answered {error.code}: {detail}") from error


def seed_evaluation(
    api: str,
    evaluation_id: str,
    suite: str,
    dataset: str,
    variant: str,
    scores: list[float],
) -> None:
    occurred_at = datetime.now(UTC).isoformat().replace("+00:00", "Z")
    events: list[dict[str, Any]] = [
        {
            "event_id": f"{evaluation_id}-started",
            "event_type": "eval.started",
            "occurred_at": occurred_at,
            "run_id": evaluation_id,
            "source": {"service": "demo-evaluator", "sdk": "python"},
            "data": {
                "suite": suite,
                "dataset": dataset,
                "variant": variant,
                "params": {"judge": "demo-rubric-v1", "temperature": "0"},
            },
        }
    ]
    for index, score in enumerate(scores, start=1):
        events.append(
            {
                "event_id": f"{evaluation_id}-case-{index}",
                "event_type": "eval.case",
                "occurred_at": occurred_at,
                "run_id": evaluation_id,
                "source": {"service": "demo-evaluator", "sdk": "python"},
                "data": {
                    "case_id": f"conversation-{index}",
                    "score": score,
                    "passed": score >= 0.7,
                    "reason": "Seeded reviewer score for dataset relation testing.",
                },
            }
        )
    passed = sum(score >= 0.7 for score in scores)
    events.append(
        {
            "event_id": f"{evaluation_id}-completed",
            "event_type": "eval.completed",
            "occurred_at": occurred_at,
            "run_id": evaluation_id,
            "source": {"service": "demo-evaluator", "sdk": "python"},
            "data": {
                "metrics": {
                    "mean_score": sum(scores) / len(scores),
                    "pass_rate": passed / len(scores),
                },
                "report": {"note": "Demo report linked to an immutable dataset version."},
            },
        }
    )
    post(f"{api}/api/v1/events", {"events": events})


def seed_volume_runs(api: str, count: int = 80) -> None:
    occurred_at = datetime.now(UTC).isoformat().replace("+00:00", "Z")
    agents = ["planner-agent", "research-agent", "reviewer-agent", "curator-agent"]
    events: list[dict[str, Any]] = []
    for index in range(1, count + 1):
        run_id = f"viewer-run-{index:03d}"
        agent = agents[(index - 1) % len(agents)]
        common = {
            "occurred_at": occurred_at,
            "run_id": run_id,
            "conversation_id": "viewer-volume",
            "agent_id": agent,
            "source": {"service": "viewer-seeder", "sdk": "python"},
        }
        events.extend(
            [
                {
                    **common,
                    "event_id": f"{run_id}-started",
                    "event_type": "run.started",
                    "data": {},
                },
                {
                    **common,
                    "event_id": f"{run_id}-completed",
                    "event_type": "run.completed",
                    "data": {"status": "succeeded"},
                },
            ]
        )
    post(f"{api}/api/v1/events", {"events": events})


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api", default="http://127.0.0.1:8080")
    parser.add_argument("--flow", default="http://127.0.0.1:8081")
    args = parser.parse_args()

    seed_volume_runs(args.api)
    print("runs: 80 lightweight rows for lazy-loading tests")

    for recipe in RECIPES:
        checked = post(f"{args.flow}/flow/check", {"pipeline": recipe["pipeline"]})
        if not checked["ok"]:
            raise RuntimeError(f"invalid recipe {recipe['name']}: {checked['diagnostics']}")
        saved = post(f"{args.api}/api/v1/curations", recipe)
        print(f"recipe: {saved['recipe']['name']}")

    result = post(f"{args.flow}/flow/query", {"pipeline": RECIPES[0]["pipeline"]})
    published = post(
        f"{args.api}/api/v1/datasets",
        {
            "name": "demo/successful-conversations",
            "description": "Seeded cases for testing Data Curation and dataset promotion.",
            "recipe": RECIPES[0]["name"],
            "pipeline": RECIPES[0]["pipeline"],
            "columns": result["columns"],
            "items": result["rows"],
            "source": result["source"],
            "window_seconds": result.get("window_seconds"),
        },
    )
    dataset = published["dataset"]
    print(f"dataset: {dataset['name']} ({dataset['latest']['row_count']} rows)")

    explorer_result = post(f"{args.flow}/flow/query", {"pipeline": RECIPES[3]["pipeline"]})
    explorer_published = post(
        f"{args.api}/api/v1/datasets",
        {
            "name": "demo/lazy-run-explorer",
            "description": "Eighty seeded run rows for TanStack Table and lazy-loading tests.",
            "recipe": RECIPES[3]["name"],
            "pipeline": RECIPES[3]["pipeline"],
            "columns": explorer_result["columns"],
            "items": explorer_result["rows"],
            "source": explorer_result["source"],
            "window_seconds": explorer_result.get("window_seconds"),
        },
    )
    explorer = explorer_published["dataset"]
    print(f"dataset: {explorer['name']} ({explorer['latest']['row_count']} rows)")
    reference = f"{dataset['name']}@{dataset['latest']['version']}"
    seed_evaluation(
        args.api,
        "eval-conversation-baseline",
        "conversation-quality",
        reference,
        "prompt-v1",
        [0.62, 0.78, 0.67],
    )
    seed_evaluation(
        args.api,
        "eval-conversation-candidate",
        "conversation-quality",
        reference,
        "prompt-v2",
        [0.81, 0.91, 0.74],
    )
    seed_evaluation(
        args.api,
        "eval-conversation-safety",
        "safety-regression",
        reference,
        "guardrail-v3",
        [0.95, 0.88, 0.92],
    )
    print(f"evaluations: 3 linked to {dataset['name']}@{dataset['latest']['version'][:12]}")


if __name__ == "__main__":
    main()
