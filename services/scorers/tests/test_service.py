"""The rules every adapter is held to, with an adapter that does what it is told."""

from __future__ import annotations

from collections.abc import Mapping

from starlette.testclient import TestClient

from aiwatcher_scorers.__main__ import refusal
from aiwatcher_scorers.adapter import Implemented
from aiwatcher_scorers.contract import Case, JsonValue, Metric, ModelReference, Parameter
from aiwatcher_scorers.service import create_app


class LeakyError(Exception):
    """An exception whose message quotes the answer it was scoring."""


def length(_: Mapping[str, JsonValue], case: Case) -> float:
    if case.answer == "explode":
        raise LeakyError("could not score 'explode', the user's secret answer")
    return float(len(str(case.answer)))


class Stub:
    name = "stub"
    version = "1.0.0"
    model: ModelReference | None = ModelReference("judge", "r1")

    def metrics(self) -> dict[str, Implemented]:
        return {
            "length": Implemented(
                Metric(
                    "length", unit="chars", direction="none", aggregation="mean", reads=("answer",)
                ),
                length,
            ),
            "graded": Implemented(
                Metric(
                    "graded",
                    unit="score",
                    direction="higher",
                    aggregation="mean",
                    reads=("input", "answer"),
                    model_graded=True,
                    parameters={"threshold": Parameter("number", required=True)},
                ),
                lambda _parameters, _case: 0.5,
            ),
        }


def client() -> TestClient:
    return TestClient(create_app([Stub()]))


def ask(
    metric: str, cases: list[dict[str, JsonValue]], **declared: JsonValue
) -> dict[str, JsonValue]:
    pinned: dict[str, JsonValue] = {"version": "1.0.0"}
    pinned.update(declared)
    return {"adapter": "stub", "metric": metric, "declared": pinned, "cases": list(cases)}


def test_replies_come_back_in_order_and_a_raising_case_says_only_its_class() -> None:
    answered = client().post(
        "/scorers/score",
        json=ask("length", [{"answer": "four"}, {"answer": "explode"}, {"answer": "ab"}]),
    )
    assert answered.status_code == 200
    replies = answered.json()["replies"]
    assert replies[0] == {"value": 4.0}
    assert replies[2] == {"value": 2.0}
    assert replies[1] == {"failed": "LeakyError: the metric raised while scoring this case"}
    assert "secret" not in answered.text


def test_a_card_pinned_against_another_release_or_model_is_a_409_naming_both() -> None:
    other_release = client().post(
        "/scorers/score", json=ask("length", [{"answer": "x"}], version="0.9.0")
    )
    assert other_release.status_code == 409
    assert "0.9.0" in other_release.json()["message"]
    assert "1.0.0" in other_release.json()["message"]

    # A graded metric is pinned with its model, and a heuristic with none.
    other_model = client().post(
        "/scorers/score",
        json={
            **ask("graded", [{"input": "q", "answer": "a"}]),
            "declared": {"version": "1.0.0", "model": {"name": "judge", "version": "r0"}},
            "parameters": {"threshold": 0.5},
        },
    )
    assert other_model.status_code == 409
    assert "judge r0" in other_model.json()["message"]
    assert "judge r1" in other_model.json()["message"]
    pinned_a_model = client().post(
        "/scorers/score",
        json=ask("length", [{"answer": "x"}], model={"name": "judge", "version": "r1"}),
    )
    assert pinned_a_model.status_code == 409


def test_an_unknown_adapter_or_metric_is_a_404_and_bad_parameters_a_422() -> None:
    assert (
        client()
        .post("/scorers/score", json={**ask("length", [{"answer": "x"}]), "adapter": "nope"})
        .status_code
        == 404
    )
    assert client().post("/scorers/score", json=ask("vibes", [{"answer": "x"}])).status_code == 404
    refused = client().post(
        "/scorers/score",
        json={
            **ask(
                "graded", [{"input": "q", "answer": "a"}], model={"name": "judge", "version": "r1"}
            ),
            "parameters": {"threshold": "high", "extra": 1},
        },
    )
    assert refused.status_code == 422
    assert (
        "`extra`" in refused.json()["message"]
        and "`threshold` is a number" in refused.json()["message"]
    )


def test_a_case_missing_a_side_the_metric_reads_fails_by_itself() -> None:
    answered = client().post(
        "/scorers/score",
        json={
            **ask(
                "graded",
                [{"answer": "a"}, {"input": "q", "answer": "a"}],
                model={"name": "judge", "version": "r1"},
            ),
            "parameters": {"threshold": 0.5},
        },
    )
    assert answered.status_code == 200
    assert answered.json()["replies"] == [
        {"failed": "the case carries no input, and this metric reads it"},
        {"value": 0.5},
    ]


def test_the_catalog_and_health_name_what_this_process_loaded() -> None:
    described = client().get("/scorers/catalog").json()
    assert described["contract"] == 1
    assert described["adapters"][0]["model"] == {"name": "judge", "version": "r1"}
    assert [metric["metric"] for metric in described["adapters"][0]["metrics"]] == [
        "graded",
        "length",
    ]
    assert client().get("/health").json() == {"status": "ok", "adapters": ["stub"]}


def test_given_a_token_the_routes_want_it_and_the_probe_does_not() -> None:
    guarded = TestClient(create_app([Stub()], token="s3cret"))
    assert guarded.get("/health").status_code == 200

    refused = guarded.get("/scorers/catalog")
    assert refused.status_code == 401
    assert "AIWATCHER_SCORER_TOKEN" in refused.json()["message"]
    assert refused.headers["www-authenticate"] == "Bearer"
    assert (
        guarded.get("/scorers/catalog", headers={"authorization": "Bearer wrong"}).status_code
        == 401
    )
    assert (
        guarded.post(
            "/scorers/score", json={}, headers={"authorization": "Bearer wrong"}
        ).status_code
        == 401
    ), "refused before the request is read"

    admitted = guarded.get("/scorers/catalog", headers={"authorization": "Bearer s3cret"})
    assert admitted.status_code == 200
    assert admitted.json()["adapters"][0]["name"] == "stub"


def test_without_a_token_it_serves_only_this_machine_unless_told_the_network_is_the_fence() -> None:
    assert refusal(None, "127.0.0.1", unauthenticated=False) is None
    assert refusal("secret", "0.0.0.0", unauthenticated=False) is None
    assert refusal(None, "0.0.0.0", unauthenticated=True) is None
    refused = refusal(None, "0.0.0.0", unauthenticated=False)
    assert refused is not None
    assert "AIWATCHER_SCORERS_TOKEN" in refused
    assert "AIWATCHER_SCORERS_UNAUTHENTICATED" in refused
