#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""A baseline and a candidate, each generating its answers and scored on one cohort (C1).

Stage C's acceptance on a small pinned set: both variants go through the whole
process, the report leads to the execution and the trace, a repeated start does
not measure twice, missing observations lower the visible coverage, and the two
results compare. The template is

    cases ──► generate (a worker's task) ──► traces ──► score

once per variant: the serve role reads the cohort's inputs under the pair's
admission, a worker answers each case with the variant it is told, and the same
score step as C0 reads what the worker wrote. The "application" is a
deterministic stand-in whose prompt decides how it answers, so the run costs
nothing and says the same thing every time:

- the **baseline** prompt answers in a sentence, and exact match fails it;
- the **candidate** prompt answers in one word;
- a second repetition of the candidate is told to decline one case.

What it checks:

1. both runs complete, four steps each, the generation on a worker;
2. the worker was handed each case's question and nothing it expected;
3. the candidate scores higher than the baseline, on the same context;
4. the comparison is `comparable`, and names the metric's rise;
5. the case the repetition declined is unscored, not a zero: coverage drops,
   and its comparison with the baseline is withheld as unverified;
6. a result names its execution and step, and a case names the trace its
   run was seen in — which the application never had to know;
7. starting a declaration again lands on the run it started, not a second one;
8. a variant pinning code the worker does not hold fails before a case is
   answered, naming both digests, and publishes nothing.
9. the experiment over that context sets the variants side by side: the
   candidate against the baseline, each with per-case latency and tokens over
   every case, and the whole run's duration from the log — and each result's
   cases priced by the model their runs' calls named, at the deployment's
   table, with the day the price was read.
10. what a variant was observed doing stands beside it: runs the application
    served naming the candidate are counted with their durations and tokens,
    while the runs it made answering the measurement's cases are counted apart
    and in no figure — and the baseline, served nowhere, is observed nowhere.
11. the traces of every generated answer showed it made on the pinned prompt,
    and the result says so: every answer seen, every one on the prompt;
12. a worker whose application renders another version of the pinned prompt
    fails at the traces step naming both versions, and publishes nothing —
    though what it generated with agrees with every pin it holds;
13. a variant that also pins a model nobody here registered and a workflow is
    seen, on every answer, executing the pinned workflow and calling the pinned
    model — and the gateway in front of the stand-in provider, publishing its own
    run under the one credential the server names a witness, is a second
    witness on every one to the model version the provider said served the call
    and to the prompt version whose template it found in the request — and, by
    its keyed digests, to every answer being the reply it relayed to a request
    that was nothing but the pinned prompt rendered with its case's question:
    the log records each run as published by the token that sent it, and none of
    those words;
14. an application that steps through a node the pinned workflow does not
    declare fails at the traces step naming the node, and publishes nothing —
    and so does one that starts `answer` before `retrieve`, which the pinned
    declaration leads into it from, has completed, and one that answers twice
    for one retrieval;
15. what the candidate was observed serving is written down as each period
    closes, and a window asked of the experiment is answered from the period
    fold alone — the written periods and the ones it holds: still five runs,
    not ten, counted from where observations began, each model call timed,
    and priced at the deployment's table, which says where and when the price
    was read;
16. a gateway holding the application's own token witnesses nothing: every
    answer is counted self-witnessed, and a gate requiring a witness holds the
    result incomplete, saying to give the serving host a token of its own;
17. an application that asks the witness about somewhere else and answers from
    a call made around it has its model and prompt witnessed and no answer
    and no question, and a gate requiring witnessed answers holds the result
    incomplete, saying they were made around the gateway;
18. an application that tells the witnessed model what to say, in a message
    beside the pinned prompt, has every answer witnessed as the reply and no
    exchange, and a gate requiring witnessed answers says why;
19. an application whose model reasons before it answers, and which says it
    takes the answer from the fenced JSON block of the reply — the steps it
    takes it out with — has every exchange witnessed: the gateway took the
    answer out of the reply the same way;
20. an application that renders the pinned prompt with a value it made beside
    the case's question has every answer witnessed as the reply and no
    exchange, and a gate requiring witnessed answers says why;
21. a request naming the pinned prompt whose text does not hold its template
    fails at the traces step on the gateway's word, and publishes nothing;
22. an application that renders the pinned prompt with the country it cut out
    of the case's question, and says in which steps, has every exchange
    witnessed: the gateway cut it out the same way;
23. an application that asks the deployment's atlas through the gateway which
    country the question is about, and renders the prompt with the country it
    took out of the atlas's answer, has every exchange witnessed: the atlas's
    word, relayed and digested by the gateway, for arguments that were the
    case's own question;
24. an application whose model answers with a label, and which takes the
    capital the label stands for, has every exchange witnessed where the
    variant's generation config pins that way of taking it — and none, with a
    gate saying why, where it does not;
25. an application that answers with the capital and the country from two
    witnessed calls, in the shape the variant's response schema pins, has every
    exchange witnessed part by part;
26. two witnessed replies joined in the words the variant pins are an exchange;
27. an answer most of three witnessed replies gave is an exchange where the
    variant pins choosing by the most of three, and a choice where it does not;
28. a country from an atlas the application called directly, witnessed on the
    atlas's host under the witness's token, is an exchange;
29. an answer a witnessed judging call named — its reply, taken out the way the
    variant pins, naming the placeholder the answer was rendered into on the
    judging prompt the variant pins — is an exchange, and the reply the judge
    did not name is a choice the application made;
30. an application that first asks each question on the pinned prompt through
    the witness in a run of no measurement's, saying nothing of what it
    rendered, has no exchange: every case was asked elsewhere, and a gate says so;
31. an atlas whose host publishes under a token of its own, digesting under the
    gateway's witness key as the deployment says it may, is an exchange;
32. an atlas the gateway answers itself, in its own process, is an exchange;
33. the server stops with runs still in a period it has not written, starts
    again on the same data and replays its log over the period fold's saved
    state: the window counts every run once — the three before the restart
    from their written period, the two after from the period the fold still
    holds open. (A restart that does not replay, as on Laser, is the projector's
    own test.) And the journal of what that fold reads has paged the log beside
    it, keeping no word said in a run;
34. a run whose every event its transport dropped is counted as lost;
35. so is the first run of a client new after the restart, whose count began
    while the fold was reading, once its second run arrives;
36. an answer a judge picked among candidates the gateway placed in the
    witnessed order the variant pins is an exchange — the application reading
    where they went from the gateway's reply;
37. candidates the application placed against that order are no exchange, and a
    gate names why;
38. a judge asked both ways, the second time reversed, is an exchange where both
    name the answer, and none where they name different replies;
39. a case asked before the measurement started — by its question alone, a part
    of the case's input — is asked elsewhere where the run reads from before its
    start (`settings.asked_since_seconds`), and not where it reads from its start;
40. so is a case asked in another case and spacing;
41. a case asked on another prompt is counted apart and denies no exchange,
    unless a gate's policy says it does;
42. a country an atlas computed in the application's own process is no
    exchange, and the trace and a gate name the atlas as where it may have come
    from;
43. an atlas the gateway answers with the code the variant pins is an exchange,
    and answered with other code the answers are refused naming both digests;
44. an answer whose run its client lost in transport is said to be lost, one
    naming a run nobody opened unknown, and a retried attempt leaves no gap;
45. cases asked long before a restart, whose calls the read model no longer
    holds, are asked elsewhere all the same, from the index of what witnesses
    saw asked;
46. a client whose transport lost its only run says so when it closes, and the
    run is counted lost; one that never closes says nothing before its clock is
    due;
47. a count a transport could not deliver is kept on its spool and sent by the
    next transport started on it, and the run lost with it is counted;
48. an atlas at the URL the deployment named, naming its code in its reply, and
    one on a host naming it where it witnesses, are held to the code the variant
    pins — an exchange on every answer, and refused naming both digests where
    the pin is another;
49. a result that read calls asked elsewhere from its start is held incomplete
    by a gate asking for ten minutes before it, and one that read from ten
    minutes before is not;
50. a client killed without closing, whose transport could not deliver its
    run, leaves on its spool the count it held as that run started, and the
    next transport there sends it: the run is counted lost;
51. a client that never closes says its count by its own clock once it is due,
    with no event after it: the run it lost is counted.

The server runs behind a stand-in authenticating proxy: a person's requests
carry its headers, and the application, the gateway and the worker each publish
with a token of their own, which is what makes the gateway's word another
credential's. The served variant's application calls a stand-in provider over
HTTP, through `aiwatcher_sdk.gateway`.

It starts **its own** aiwatcher, from `target/debug/aiwatcher` or
`AIWATCHER_BINARY`, on a free port with every byte under a temporary directory,
so the one on :8080 is not touched.

    cargo build --bin aiwatcher   # once
    just e2e-generate
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

import aiwatcher_sdk  # noqa: E402
from aiwatcher_sdk import (  # noqa: E402
    CALLER_RUN_HEADER,
    PLACED_HEADER,
    PROMPT_HEADER,
    TOOL_CODE_HEADER,
    AiwatcherClient,
    HttpTransport,
)
from aiwatcher_sdk.gateway import (  # noqa: E402
    Gateway,
    ToolWitness,
    extracted,
    tool_code,
    witness_digest,
    witness_key,
)
from aiwatcher_sdk.prompts import PromptRegistry  # noqa: E402
from aiwatcher_sdk.task_errors import TaskError  # noqa: E402
from aiwatcher_sdk.worker import (  # noqa: E402
    Case,
    Declined,
    Generated,
    GeneratedWith,
    Generation,
    JsonValue,
    Worker,
    generation_task,
    get_task_context,
)

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", ROOT / "target" / "debug" / "aiwatcher"))
READY_WITHIN = 30.0
RUN_WITHIN = 90.0
BASE = ""

QUEUE = "e2e-generate"

#: Three credentials, as three hosts hold them: the application's, a model
#: server's, and the worker's, which may claim this queue's work.
APPLICATION_SECRET = "a1" * 16
SERVING_SECRET = "5e" * 16
WORKER_SECRET = "0b" * 16
#: The atlas's host's own token, digesting under the witness's key.
ATLAS_SECRET = "a7" * 16
#: What a person's request carries from the proxy in front of the server.
PERSON = {
    "x-authentik-uid": "e2e",
    "x-authentik-username": "e2e",
    "x-authentik-groups": "aiwatcher-admins",
}

#: The pinned workflow's declaration, and the model a model server serves.
WORKFLOW = json.dumps({"nodes": ["retrieve", "answer"], "edges": [["retrieve", "answer"]]}).encode()
WEIGHTS = b"the capitals model's weights\n"
PACKAGE = json.dumps(
    {
        "runtime": "weights",
        "artifacts": [
            {
                "name": "weights",
                "uri": "s3://elsewhere/capitals.bin",
                "digest": hashlib.sha256(WEIGHTS).hexdigest(),
                "size_bytes": len(WEIGHTS),
                "content_type": "",
                "kind": "model",
            }
        ],
    }
).encode()
MODEL = {"name": "capitals-served", "version": hashlib.sha256(PACKAGE).hexdigest()}
PROMPT = "e2e.capitals"
DATASET = "capitals"
TERMINAL = {"completed", "failed", "cancelled", "crashed"}

CAPITALS = (
    ("France", "Paris"),
    ("Japan", "Tokyo"),
    ("Peru", "Lima"),
    ("Kenya", "Nairobi"),
)

PROMPTS = {
    "baseline": "Answer this question helpfully: {{ question }}",
    "candidate": "Answer this question in one word: {{ question }}",
    # The country cut out of the question, or out of the atlas's answer.
    "cut": "Name the capital of {{ country }} in one word.",
    "labelled": "Answer this question in one word: {{ question }}",
    "labelled-loose": "Answer this question in one word: {{ question }}",
    "composed": "Answer this question in one word: {{ question }}",
    "joined": "Answer this question in one word: {{ question }}",
    "chosen": "Answer this question in one word: {{ question }}",
    "chosen-loose": "Answer this question in one word: {{ question }}",
    "judged": "Answer this question in one word: {{ question }}",
    "judged-witnessed": "Answer this question in one word: {{ question }}",
    "judged-both": "Answer this question in one word: {{ question }}",
    "cut-in-app": "Name the capital of {{ country }} in one word.",
    "cut-pinned": "Name the capital of {{ country }} in one word.",
    "cut-mispinned": "Name the capital of {{ country }} in one word.",
    "cut-url-pinned": "Name the capital of {{ country }} in one word.",
    "cut-hosted-pinned": "Name the capital of {{ country }} in one word.",
    "cut-url-mispinned": "Name the capital of {{ country }} in one word.",
    "counted": "Answer this question in one word: {{ question }}",
    # Each its own text, so only the calls a check makes ask on it.
    "asked-before": "Answer in one word, if you would: {{ question }}",
    "asked-unread": "Answer in one word, if you please: {{ question }}",
    "asked-normal": "Answer in one word, plainly: {{ question }}",
    "asked-unpinned": "Answer in one word, on the record: {{ question }}",
    "asked-restart": "Answer in one word, once more: {{ question }}",
}
#: Another prompt, asked the same questions.
CHAT_PROMPT = "e2e.chat"
CHAT = "Chat about this: {{ question }}"
#: The prompt a judging call is asked on, and how its answer is taken out.
JUDGE_PROMPT = "e2e.judge"
JUDGE = "Which answers {{ question }} rightly, {{ first }} or {{ second }}? Reply in JSON."
JUDGE_VERSION = hashlib.sha256(JUDGE.encode()).hexdigest()
PICK: dict[str, Any] = {"json_pointer": "/best"}

#: The label a model answers each capital with, and the way the labelled
#: variants take the capital back out — which only "labelled" pins.
LABELS = {f"L{at}": capital for at, (_, capital) in enumerate(CAPITALS, start=1)}
TAKING_LABELS: dict[str, Any] = {"steps": [{"strip": "."}, {"map": LABELS}]}
#: The shape of the composed variant's answer.
RESPONSE_SCHEMA = json.dumps(
    {
        "type": "object",
        "properties": {"capital": {"type": "string"}, "country": {"type": "string"}},
    }
).encode()

#: What the worker was handed, to check nothing it expected reached it.
HANDED: list[dict[str, Any]] = []

#: The application's own telemetry, pointed at the server once it is up.
TELEMETRY: list[AiwatcherClient] = []
#: The gateways in front of the stand-in provider: one holding a witness's
#: token, and one holding the application's own.
GATEWAYS: dict[str, str] = {}
#: The atlas's host witnessing its own calls, under the witness's token.
WITNESSED_ATLAS: dict[str, Any] = {}


class Provider(BaseHTTPRequestHandler):
    """A stand-in model provider: one word when its system message asks for one —
    reasoning first, then the answer in a fenced JSON block, when the request's
    ``user`` says explain."""

    def log_message(self, format: str, *args: Any) -> None:
        del format, args

    def do_POST(self) -> None:
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        system, question = (message["content"] for message in body["messages"][:2])
        # Whichever country the question names, however it is spelled.
        country = next(name for name, _ in CAPITALS if name.lower() in question.lower())
        said = application(system, country, dict(CAPITALS)[country])
        if body.get("user") == "label":
            said = next(label for label, capital in LABELS.items() if capital == said) + "."
        if body.get("user") == "country":
            said = country
        if body.get("user") == "judge":
            # The judge names the first candidate it was shown.
            said = json.dumps({"best": "first"})
        if body.get("user") == "judge-right":
            # The judge names the candidate that is the capital, wherever it stands.
            shown = system.split("rightly, ", 1)[1].rsplit("? Reply", 1)[0]
            first, _ = shown.split(" or ", 1)
            said = json.dumps({"best": "first" if first == dict(CAPITALS)[country] else "second"})
        if body.get("user") == "other":
            # Another capital than the one asked about: a reply to choose against.
            at = [name for name, _ in CAPITALS].index(country)
            said = CAPITALS[(at + 1) % len(CAPITALS)][1]
        if body.get("user") == "explain":
            said = (
                f"It is where the government of {country} sits.\n"
                f"```json\n{json.dumps({'capital': said, 'confidence': 0.9})}\n```"
            )
        payload = json.dumps(
            {
                # The version it served, as a provider names its snapshot.
                "model": MODEL["version"],
                "choices": [{"message": {"role": "assistant", "content": said}}],
                "usage": {
                    "prompt_tokens": len(question.split()),
                    "completion_tokens": len(said.split()),
                },
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class Atlas(BaseHTTPRequestHandler):
    """A tool the deployment runs: which country a question is about."""

    def log_message(self, format: str, *args: Any) -> None:
        del format, args

    def do_POST(self) -> None:
        asked = json.loads(self.rfile.read(int(self.headers["content-length"])))
        country = str(asked["question"]).removeprefix("What is the capital of ").removesuffix("?")
        payload = json.dumps({"country": country}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        # The code that answered, which the gateway publishes as this service's word.
        self.send_header(TOOL_CODE_HEADER, tool_code(Atlas.do_POST) or "")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class WitnessedAtlas(Atlas):
    """The same atlas, witnessing each call where it runs rather than through a gateway."""

    witnessed_by = "witness"

    def do_POST(self) -> None:
        raw = self.rfile.read(int(self.headers["content-length"]))
        asked = json.loads(raw)
        country = str(asked["question"]).removeprefix("What is the capital of ").removesuffix("?")
        payload = json.dumps({"country": country}).encode()
        witness: ToolWitness = WITNESSED_ATLAS[self.witnessed_by]
        with witness.call(
            "atlas",
            asked,
            caller=self.headers.get(CALLER_RUN_HEADER),
            code=tool_code(Atlas.do_POST),
        ) as call:
            call.answered(payload)
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class HostedAtlas(WitnessedAtlas):
    """The atlas again, on a host publishing under its own token with the witness's key."""

    witnessed_by = "hosted"


def atlas_here(asked: Any) -> dict[str, str]:
    """The atlas as a function the gateway answers with in its own process."""
    return {
        "country": str(asked["question"]).removeprefix("What is the capital of ").removesuffix("?")
    }


def through_gateway(
    which: str,
    headers: dict[str, str],
    system: str,
    question: str,
    body: dict[str, Any] | None = None,
    replied_headers: dict[str, str] | None = None,
) -> dict[str, Any]:
    """One call to the provider, through a gateway — or, for `around`, straight to it."""
    request = urllib.request.Request(  # noqa: S310 — the e2e's own gateway
        GATEWAYS[which] + "/v1/chat/completions",
        data=json.dumps(
            {
                "model": MODEL["name"],
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": question},
                ],
                **(body or {}),
            }
        ).encode(),
        method="POST",
    )
    request.add_header("content-type", "application/json")
    for name, value in headers.items():
        request.add_header(name, value)
    with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310
        reply: dict[str, Any] = json.loads(response.read())
        if replied_headers is not None:
            replied_headers.update(response.headers.items())
    return reply


#: The key a gateway holding the witness's token digests under — which this e2e,
#: as the deployment, holds, and the application never does.
WITNESS_KEY = witness_key(SERVING_SECRET)
#: What the atlas the gateway answers itself is: this file.
ATLAS_CODE = tool_code(atlas_here)
#: The telemetry of an attempt whose transport loses one case's run.
LOSSY: dict[str, AiwatcherClient] = {}


class DroppingPrefix:
    """A transport that loses every event of the runs whose ID starts so."""

    def __init__(self, inner: HttpTransport, prefix: str) -> None:
        self.inner = inner
        self.prefix = prefix

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.inner.send([event for event in batch if not event["run_id"].startswith(self.prefix)])

    def flush(self) -> None:
        self.inner.flush()

    def close(self) -> None:
        self.inner.close()


class Dropping:
    """A transport that loses every event of one run, as an outage would."""

    def __init__(self, inner: HttpTransport, run_id: str) -> None:
        self.inner = inner
        self.run_id = run_id

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.inner.send([event for event in batch if event["run_id"] != self.run_id])

    def flush(self) -> None:
        self.inner.flush()

    def close(self) -> None:
        self.inner.close()


# ── The application, and the task a worker hosts. ────────────────────────────


def application(prompt: str, country: str, capital: str) -> str:
    """A model that answers in one word only when it is told to."""
    if "one word" in prompt:
        return capital
    return f"The capital of {country} is {capital}, as it happens."


def code_of(which: str) -> bytes:
    return f"# the {which} application\n".encode()


def generation_of(which: str) -> bytes:
    config: dict[str, Any] = {"temperature": 0, "variant": which}
    if which == "labelled":
        config["answer_from"] = TAKING_LABELS
    if which == "joined":
        config["answer_joined"] = {"separator": ", "}
    if which == "chosen":
        config["answer_chosen"] = {"most_of": 3}
    if which.startswith("judged"):
        judged: dict[str, Any] = {
            "prompt": {"name": JUDGE_PROMPT, "version": JUDGE_VERSION},
            "pick": PICK,
        }
        if which == "judged-witnessed":
            judged["order"] = "witnessed"
        if which == "judged-both":
            judged["order"] = "both_ways"
        config["answer_chosen"] = {"judged": judged}
    if which == "cut-pinned":
        config["tool_code"] = {"atlas-here": ATLAS_CODE}
    if which == "cut-mispinned":
        config["tool_code"] = {"atlas-here": hashlib.sha256(b"# another atlas\n").hexdigest()}
    if which in ("cut-url-pinned", "cut-hosted-pinned"):
        config["tool_code"] = {"atlas": ATLAS_CODE}
    if which == "cut-url-mispinned":
        config["tool_code"] = {"atlas": hashlib.sha256(b"# another atlas\n").hexdigest()}
    return json.dumps(config).encode()


def holding(run: Generation) -> GeneratedWith:
    """What this worker was built with for the variant it is asked about."""
    which = str(run.variant["experiment_id"])
    return GeneratedWith.of(
        code=code_of(which),
        generation_config=generation_of(which),
        response_schema=RESPONSE_SCHEMA if which == "composed" else None,
    )


@generation_task("e2e.capitals.answer", version="1", generated_with=holding)
def answer(case: Case, run: Generation) -> JsonValue | Generated | Declined:
    context = get_task_context()
    HANDED.extend(context.read_artifact("cases") if not HANDED else [])
    prompt = run.variant["prompt"]
    assert isinstance(prompt, dict)
    with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
        version = prompts.get_version(str(prompt["name"]), str(prompt["version"]))
    text = version.text
    assert isinstance(case.input, dict)
    question = str(case.input["question"])
    country = question.removeprefix("What is the capital of ").removesuffix("?")
    capital = dict(CAPITALS)[country]
    if run.params.get("decline") == country:
        return Declined("the application would not say")
    if run.params.get("peek"):
        # The question first, on the pinned prompt through the witness, for no
        # run of the measurement's — saying nothing of what it rendered.
        through_gateway(
            "witness",
            {PROMPT_HEADER: f"{prompt['name']}@{prompt['version']}"},
            text.replace("{{ question }}", question),
            question,
        )
    # The application's run for this case: the variant's, and a measurement's —
    # and the prompt version it renders, which is the variant's unless the run
    # was told to render another.
    rendered = str(run.params.get("render_version") or prompt["version"])
    if rendered != prompt["version"]:
        with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
            version = prompts.get_version(str(prompt["name"]), rendered)
        text = version.text
    model = run.variant.get("model")
    if isinstance(model, dict):
        # The application as an execution of the workflow the variant pins,
        # calling the pinned model through a gateway that reports its own runs
        # — with the prompt rendered, unless the run was told to drift from it.
        steps = ["retrieve", "answer"]
        composed: JsonValue | None = None
        if run.params.get("stray"):
            steps.append("improvise")
        if run.params.get("reorder"):
            steps.reverse()
        if run.params.get("twice"):
            steps.append("answer")
        with run.traced_workflow(
            TELEMETRY[0],
            case,
            "capitals-app",
            nodes=["retrieve", "answer"],
            edges=[("retrieve", "answer")],
        ) as flow:
            for node in steps:
                if node == "retrieve":
                    with flow.node(node):
                        continue
                with (
                    flow.node(node) as stage,
                    stage.agent("capitals") as agent,
                    agent.llm(
                        model=str(model["name"]), prompt=(str(prompt["name"]), rendered)
                    ) as llm,
                ):
                    cut_from = {"between": ["capital of ", "?"]}
                    found = ""
                    looked_up = next(
                        (
                            way
                            for way in (
                                "atlas",
                                "witnessed_atlas",
                                "hosted_atlas",
                                "gateway_atlas",
                            )
                            if run.params.get(way)
                        ),
                        None,
                    )
                    if looked_up:
                        # Which country, from the deployment's atlas, asked
                        # through the gateway with the case's own question —
                        # or of the atlas itself, which witnesses its own calls
                        # under the witness's token or its own, or answered by
                        # the gateway in its own process.
                        atlas = urllib.request.Request(  # noqa: S310 — the e2e's own gateway
                            {
                                "atlas": GATEWAYS["witness"] + "/tools/atlas",
                                "witnessed_atlas": WITNESSED_ATLAS["url"],
                                "hosted_atlas": WITNESSED_ATLAS["hosted_url"],
                                "gateway_atlas": GATEWAYS["witness"] + "/tools/atlas-here",
                            }[looked_up],
                            data=json.dumps({"question": question}).encode(),
                            method="POST",
                        )
                        for name, value in llm.caller_headers().items():
                            atlas.add_header(name, value)
                        with urllib.request.urlopen(atlas, timeout=15) as response:  # noqa: S310
                            found = response.read().decode()
                    if run.params.get("atlas_in_app"):
                        # The atlas computed here, in the application's own
                        # process: its telemetry says so, and nothing witnesses it.
                        with agent.tool("atlas"):
                            found = json.dumps(atlas_here({"question": question}))
                    named = extracted(found, {"json_pointer": "/country"}) if found else country
                    system = (
                        "Answer in one word."
                        if run.params.get("drift")
                        else version.render(country=named)
                        if run.params.get("cut") or looked_up or run.params.get("atlas_in_app")
                        else version.render(question=question)
                    )
                    if run.params.get("around"):
                        # Somewhere else, through the witness; this case, around it.
                        elsewhere = (
                            f"What is the capital of {'Kenya' if country != 'Kenya' else 'Japan'}?"
                        )
                        through_gateway(
                            "witness",
                            llm.caller_headers(),
                            version.render(question=elsewhere),
                            elsewhere,
                            llm.caller_body(question=elsewhere),
                        )
                    # How the application takes its answer out of a reasoned
                    # reply, which the gateway is told before the reply comes.
                    taking = {"steps": [{"fenced": "json"}, {"json_pointer": "/capital"}]}
                    # A value the application made, beside the case's own.
                    hint = f"It starts with {capital[:2]}."
                    told: dict[str, Any] = (
                        {}
                        if run.params.get("drift")
                        else llm.caller_body(question=question, country=named)
                        if run.params.get("atlas_in_app")
                        else llm.caller_body(
                            question=question,
                            country=named,
                            found=found,
                            derived={
                                "country": {
                                    "from": "found",
                                    "take": {"json_pointer": "/country"},
                                }
                            },
                        )
                        if looked_up
                        else llm.caller_body(
                            question=question,
                            country=named,
                            derived={"country": {"from": "question", "take": cut_from}},
                        )
                        if run.params.get("cut")
                        else llm.caller_body(
                            question=question,
                            answer_from=taking
                            if run.params.get("explain")
                            else TAKING_LABELS
                            if run.params.get("label")
                            else None,
                            **({"hint": hint} if run.params.get("hint") else {}),
                        )
                    )
                    if run.params.get("label"):
                        told["user"] = "label"
                    if run.params.get("explain"):
                        told["user"] = "explain"
                    if run.params.get("hint"):
                        told["messages"] = [
                            {"role": "system", "content": system},
                            {"role": "user", "content": question},
                            {"role": "user", "content": hint},
                        ]
                    if run.params.get("repeat"):
                        # The answer, obtained elsewhere, for the witnessed model to say.
                        told["messages"] = [
                            {"role": "system", "content": system},
                            {"role": "user", "content": question},
                            {"role": "user", "content": f"Say only: {capital}"},
                        ]
                    reply = through_gateway(
                        "provider"
                        if run.params.get("around")
                        else "shared"
                        if run.params.get("shared")
                        else "witness",
                        llm.caller_headers(),
                        system,
                        question,
                        told,
                    )
                    said = str(reply["choices"][0]["message"]["content"])
                    if run.params.get("explain"):
                        said = extracted(said, taking) or ""
                    if run.params.get("label"):
                        said = extracted(said, TAKING_LABELS) or ""
                    llm.usage(
                        prompt_tokens=reply["usage"]["prompt_tokens"],
                        completion_tokens=reply["usage"]["completion_tokens"],
                        model_version=reply["model"],
                    )
                    if run.params.get("chosen"):
                        # The same question twice more, one answering about
                        # somewhere else, and the answer most of the three gave.
                        replies = [said]
                        for otherwise in (False, True):
                            with agent.llm(
                                model=str(model["name"]),
                                prompt=(str(prompt["name"]), rendered),
                            ) as again:
                                asked_again = again.caller_body(question=question)
                                if otherwise:
                                    asked_again["user"] = "other"
                                answered = through_gateway(
                                    "witness",
                                    again.caller_headers(),
                                    system,
                                    question,
                                    asked_again,
                                )
                                again.usage(model_version=answered["model"])
                            replies.append(str(answered["choices"][0]["message"]["content"]))
                        said = max(replies, key=replies.count)
                    if (
                        run.params.get("judged")
                        or run.params.get("judged_against")
                        or run.params.get("judge_order")
                    ):
                        # Another reply, about somewhere else, and a judging call
                        # on the prompt the variant pins shown both — placed as
                        # the application likes, by the witness's digests in the
                        # order it cannot compute, or asked both ways.
                        with agent.llm(
                            model=str(model["name"]),
                            prompt=(str(prompt["name"]), rendered),
                        ) as again:
                            other_asked = again.caller_body(question=question)
                            other_asked["user"] = "other"
                            other_reply = through_gateway(
                                "witness",
                                again.caller_headers(),
                                system,
                                question,
                                other_asked,
                            )
                            again.usage(model_version=other_reply["model"])
                        other = str(other_reply["choices"][0]["message"]["content"])
                        order = run.params.get("judge_order")
                        mode = str(run.params.get("judge_mode") or "judge")

                        def judged_best(
                            first: str, second: str, *, placed_by_gateway: bool, mode: str = mode
                        ) -> str:
                            with (
                                PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts,
                                agent.llm(
                                    model=str(model["name"]),
                                    prompt=(JUDGE_PROMPT, JUDGE_VERSION),
                                ) as judge,
                            ):
                                judging = prompts.get_version(JUDGE_PROMPT, JUDGE_VERSION).render(
                                    question=question, first=first, second=second
                                )
                                asked_judge = judge.caller_body(
                                    question=question,
                                    first=first,
                                    second=second,
                                    answer_from=PICK,
                                    ordered=("first", "second") if placed_by_gateway else (),
                                )
                                asked_judge["user"] = mode
                                replied: dict[str, str] = {}
                                verdict = through_gateway(
                                    "witness",
                                    judge.caller_headers(),
                                    judging,
                                    question,
                                    asked_judge,
                                    replied,
                                )
                                judge.usage(model_version=verdict["model"])
                            named_best = extracted(
                                str(verdict["choices"][0]["message"]["content"]), PICK
                            )
                            # Where the gateway placed each, as its reply says.
                            placed = json.loads(replied.get(PLACED_HEADER) or "{}")
                            values = {"first": first, "second": second}
                            return values[placed.get(named_best, named_best or "first")]

                        if order == "both":
                            best = judged_best(said, other, placed_by_gateway=False)
                            judged_best(other, said, placed_by_gateway=False)
                        elif order == "descending":
                            # The candidates by their digests, highest first: the
                            # order a witnessed judge must not be shown.
                            high, low = sorted(
                                (said, other),
                                key=lambda value: witness_digest(WITNESS_KEY, "replied", value),
                                reverse=True,
                            )
                            best = judged_best(high, low, placed_by_gateway=False)
                        else:
                            best = judged_best(said, other, placed_by_gateway=order == "gateway")
                        said = (
                            (other if best == said else said)
                            if run.params.get("judged_against")
                            else best
                        )
                    if run.params.get("composed") or run.params.get("joined"):
                        # A second witnessed call in the same stage, for the
                        # country, and an answer made of both replies.
                        with agent.llm(
                            model=str(model["name"]),
                            prompt=(str(prompt["name"]), rendered),
                        ) as country_call:
                            second = country_call.caller_body(question=question)
                            second["user"] = "country"
                            answered = through_gateway(
                                "witness",
                                country_call.caller_headers(),
                                system,
                                question,
                                second,
                            )
                            country_call.usage(model_version=answered["model"])
                        named_back = str(answered["choices"][0]["message"]["content"])
                        composed = (
                            {"capital": said, "country": named_back}
                            if run.params.get("composed")
                            else f"{said}, {named_back}"
                        )
        if composed is not None:
            return Generated(composed, run_id=flow.correlation.run_id)
        return Generated(said, run_id=flow.correlation.run_id)
    if run.params.get("fail_first") and run.attempt == 1 and country == "Japan":
        # The provider goes away mid-attempt, after one case's run was opened.
        raise TaskError("the provider went away", classification="transient")
    telemetry = TELEMETRY[0]
    if run.params.get("lose_run"):
        # One client for the attempt, whose transport loses France's run whole.
        telemetry = LOSSY.setdefault(
            run.evaluation_id,
            AiwatcherClient(
                service="e2e-capitals",
                transport=DroppingPrefix(
                    HttpTransport(BASE, token=APPLICATION_SECRET),
                    f"generate-{run.evaluation_id}-capital-france-",
                ),
            ),
        )
    with (
        run.traced(telemetry, case) as traced,
        traced.agent("capitals") as agent,
        agent.llm(model="capitals-stand-in", prompt=(str(prompt["name"]), rendered)) as llm,
    ):
        said = application(text, country, capital)
        llm.usage(prompt_tokens=len(question.split()), completion_tokens=len(said.split()))
    # What a model would have counted: the question in, the words out.
    return Generated(
        said,
        run_id="invented-run"
        if run.params.get("invent") and country == "France"
        else traced.correlation.run_id,
        input_tokens=len(question.split()),
        output_tokens=len(said.split()),
    )


# ── The server. ──────────────────────────────────────────────────────────────


#: The day the stand-in's current price was read, and the day an older one was.
PRICED_ON = time.strftime("%Y-%m-%d", time.gmtime())
PRICED_BEFORE = time.strftime("%Y-%m-%d", time.gmtime(time.time() - 30 * 86_400))


def prices(home: Path) -> Path:
    """The deployment's price table: the stand-in model, per million tokens — a
    history of two entries, the older of which prices nothing measured today."""
    table = home / "prices.json"
    table.write_text(
        json.dumps(
            {
                "currency": "USD",
                "prices": [
                    {
                        "model": "capitals-stand-in",
                        "input_per_million": 9.0,
                        "output_per_million": 9.0,
                        "source": "https://example.com/capitals-stand-in/pricing",
                        "as_of": PRICED_BEFORE,
                    },
                    {
                        "model": "capitals-stand-in",
                        "input_per_million": 1.0,
                        "output_per_million": 2.0,
                        "source": "https://example.com/capitals-stand-in/pricing",
                        "as_of": PRICED_ON,
                    },
                ],
            }
        )
    )
    return table


def serve(
    home: Path, port: int | None = None, extra: dict[str, str] | None = None
) -> subprocess.Popen[bytes]:
    """Start an aiwatcher of our own and wait for it — on `port` again, after a restart."""
    global BASE
    if not BINARY.exists():
        raise SystemExit(
            f"no server binary at {BINARY}: `cargo build --bin aiwatcher`, "
            "or name one with AIWATCHER_BINARY"
        )
    if port is None:
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
    home.mkdir(parents=True, exist_ok=True)
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
        # The worker is another thread claiming over HTTP, and the `file` store
        # refuses a plan that needs one.
        "AIWATCHER_WORKFLOW_STORE": "memory",
        "AIWATCHER_INGEST_ENABLED": "true",
        "AIWATCHER_SEED_FILE": "none",
        "AIWATCHER_LOG": "warn",
        # Periods of five seconds, so one closes while this runs; and a price
        # for the stand-in model, read from a page on a day.
        "AIWATCHER_OBSERVATION_PERIOD_SECONDS": "5",
        # A journal of what the period fold reads, which a gap is refilled from.
        "AIWATCHER_OBSERVATION_JOURNAL_DAYS": "1",
        "AIWATCHER_MODEL_PRICES": str(prices(home)),
        # The credentials whose runs witness a generated answer — the atlas's
        # host among them, digesting under the gateway's key.
        "AIWATCHER_WITNESSES": "serving,atlas",
        "AIWATCHER_WITNESS_DIGESTS": "atlas=serving",
        "AIWATCHER_AUTH_MODE": "proxy",
        "AIWATCHER_AUTH_INGEST_TOKENS": ",".join(
            [
                f"application={APPLICATION_SECRET}",
                f"serving={SERVING_SECRET}",
                f"atlas={ATLAS_SECRET}",
                f"worker[{QUEUE}]={WORKER_SECRET}",
            ]
        ),
        **(extra or {}),
    }
    log = (home / "server.log").open("wb")
    process = subprocess.Popen(  # noqa: S603 — the binary this repository builds
        [str(BINARY)], cwd=home, env=env, stdout=log, stderr=subprocess.STDOUT
    )
    BASE = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + READY_WITHIN
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        if call("GET", "/readyz")[0] == 200 and call("GET", "/api/v1/prompts")[0] == 200:
            return process
        time.sleep(0.2)
    process.kill()
    tail = (home / "server.log").read_text(errors="replace")[-2000:]
    raise SystemExit(f"aiwatcher did not come up on {BASE}:\n{tail}")


def call(
    method: str, path: str, body: Any = None, *, raw: bytes | None = None
) -> tuple[int, Any, bytes]:
    data = raw if raw is not None else None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(  # noqa: S310 — our own server
        BASE + path, data=data, method=method, headers=PERSON
    )
    request.add_header(
        "Content-Type",
        "application/octet-stream" if raw is not None else "application/json",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — our own server
            content = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        content, status = error.read(), error.code
    except (urllib.error.URLError, ConnectionError):
        return 0, None, b""
    try:
        parsed = json.loads(content) if content else None
    except json.JSONDecodeError:
        parsed = None
    return status, parsed, content


def ok(status: int, body: Any, what: str) -> Any:
    if status not in (200, 201, 202, 204):
        raise SystemExit(f"{what} answered {status}: {json.dumps(body)[:600]}")
    return body


# ── What both variants share: a dataset version, a cohort and a card. ────────


def artifact(name: str, content: bytes) -> dict[str, Any]:
    return {
        "name": name,
        "uri": f"file://{name}",
        "digest": hashlib.sha256(content).hexdigest(),
        "size_bytes": len(content),
        "content_type": "application/json",
    }


def shared() -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    rows = [
        {
            "case_id": f"capital-{country.lower()}",
            "input": {"question": f"What is the capital of {country}?"},
            "expected": {"answer": capital},
        }
        for country, capital in CAPITALS
    ]
    published = ok(
        *call(
            "POST",
            "/api/v1/datasets",
            {
                "name": DATASET,
                "pipeline": "data_frame()->read(capitals)",
                "columns": ["case_id", "input", "expected"],
                "items": rows,
                "source": hashlib.sha256(json.dumps(rows).encode()).hexdigest(),
            },
        )[:2],
        "publishing the dataset version",
    )
    dataset = {
        "kind": "curation",
        "name": DATASET,
        "version": published["dataset"]["latest"]["version"],
    }
    derived = ok(
        *call("POST", "/api/v1/evaluation-cohorts", {"dataset": dataset, "split": "test"})[:2],
        "deriving the cohort",
    )
    card = ok(
        *call(
            "POST",
            "/api/v1/evaluation-scorecards",
            {
                "name": "capitals-exact",
                "scorers": [
                    {
                        "metric": "exact",
                        "expected_path": "/answer",
                        "scorer": {"kind": "exact_match", "trim": True},
                    }
                ],
            },
        )[:2],
        "publishing the card",
    )
    return (
        dataset,
        derived["cohort"],
        {"name": card["scorecard"]["name"], "version": card["version"]},
    )


def declare(
    which: str,
    dataset: dict[str, Any],
    cohort: dict[str, Any],
    card: dict[str, Any],
    *,
    repetition: str = "measurement-1",
    params: dict[str, Any] | None = None,
    code: bytes | None = None,
    served: bool = False,
    suffix: str = "",
    settings: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Declare one variant's run, admit its pair, and return the view.

    ``served`` also pins the model a model server serves and the workflow the
    application executes, and stages what those imply beside the rest.
    """
    with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
        version = prompts.publish(PROMPT, PROMPTS[which], author="e2e").version_id
        if (
            which.startswith("judged")
            and prompts.publish(JUDGE_PROMPT, JUDGE, author="e2e").version_id != JUDGE_VERSION
        ):
            raise SystemExit("the judging prompt's version is not the digest of its text")
    code = code_of(which) if code is None else code
    generation = generation_of(which)
    run = {
        "evaluation_id": f"capitals-{which}"
        + ("" if repetition == "measurement-1" else f"-{repetition}")
        + ("" if code == code_of(which) else "-unheld")
        + suffix,
        "repetition_id": repetition,
        "variant": {
            "schema_version": 1,
            "experiment_id": which,
            "dataset": dataset,
            "prompt": {"name": PROMPT, "version": version},
            "code": artifact("application.py", code),
            "generation_config": artifact("generation.json", generation),
        },
        "cohort": cohort,
        "scorecard": card,
        "answers": {
            "generated_by": {
                "task": "e2e.capitals.answer@1",
                "queue": QUEUE,
                **({"params": params} if params else {}),
            }
        },
        "settings": {"timeout_seconds": 300, **(settings or {})},
    }
    staged = [("application.py", code), ("generation.json", generation)]
    if which == "composed":
        run["variant"]["response_schema"] = artifact("response.json", RESPONSE_SCHEMA)
        staged.append(("response.json", RESPONSE_SCHEMA))
    if served:
        run["variant"]["model"] = MODEL
        run["variant"]["workflow"] = {
            "name": "capitals-app",
            "version": hashlib.sha256(WORKFLOW).hexdigest(),
        }
        staged += [
            ("model-package.json", PACKAGE),
            ("model-artifacts/weights", WEIGHTS),
            ("workflow.json", WORKFLOW),
        ]
    view = ok(*call("POST", "/api/v1/evaluation-runs", run)[:2], f"declaring the {which} run")
    approval = view["approval_id"]
    # The operator's act: the manifest and the files the variant pins. The
    # cohort's three are derived again from the dataset version, never staged.
    for name, content in (
        ("manifest.json", json.dumps(view["manifest"]).encode()),
        *staged,
    ):
        ok(
            *call(
                "PUT",
                f"/api/v1/evaluation-approvals/{approval}/bundle/{name}",
                raw=content,
            )[:2],
            f"staging {name}",
        )
    ok(
        *call("POST", "/api/v1/evaluation-approvals", view["manifest"])[:2],
        f"admitting the {which} pair",
    )
    return view


def followed(execution_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + RUN_WITHIN
    while time.monotonic() < deadline:
        status, body, _ = call("GET", f"/api/v1/executions/{execution_id}")
        if status == 200 and body["execution"]["state"]["state_type"] in TERMINAL:
            return body
        time.sleep(0.2)
    raise SystemExit(f"{execution_id} did not finish within {RUN_WITHIN:.0f}s")


# ── The run. ─────────────────────────────────────────────────────────────────


def main() -> int:
    failures: list[str] = []

    def check(number: int, claim: str, holds: bool, detail: object = "") -> None:
        mark = "✓" if holds else "✗"
        print(f"  {mark} {number}. {claim}" + (f" — {detail}" if detail not in ("", None) else ""))
        if not holds:
            failures.append(claim)

    home = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-generate-"))
    server = serve(home / "server")
    # When the fold could first have read anything: a client's count beginning
    # within a minute of it is one a clock running ahead could be dating late.
    reading_since = time.time()
    TELEMETRY.append(
        AiwatcherClient(service="e2e-capitals", base_url=BASE, token=APPLICATION_SECRET)
    )
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    atlas = ThreadingHTTPServer(("127.0.0.1", 0), Atlas)
    threading.Thread(target=atlas.serve_forever, daemon=True).start()
    witnessed_atlas = ThreadingHTTPServer(("127.0.0.1", 0), WitnessedAtlas)
    threading.Thread(target=witnessed_atlas.serve_forever, daemon=True).start()
    hosted_atlas = ThreadingHTTPServer(("127.0.0.1", 0), HostedAtlas)
    threading.Thread(target=hosted_atlas.serve_forever, daemon=True).start()
    # The witness's own token, on the atlas's host: the gateway's key, so its
    # digests are the ones a call the gateway relayed is rendered with.
    WITNESSED_ATLAS["witness"] = ToolWitness(
        AiwatcherClient(service="e2e-atlas", base_url=BASE, token=SERVING_SECRET),
        credential=SERVING_SECRET,
    )
    WITNESSED_ATLAS["url"] = f"http://127.0.0.1:{witnessed_atlas.server_address[1]}/atlas"
    # A host of its own, with its own token — and the gateway's witness key,
    # which the deployment says its digests are made under.
    WITNESSED_ATLAS["hosted"] = ToolWitness(
        AiwatcherClient(service="e2e-atlas-host", base_url=BASE, token=ATLAS_SECRET),
        key=witness_key(SERVING_SECRET),
    )
    WITNESSED_ATLAS["hosted_url"] = f"http://127.0.0.1:{hosted_atlas.server_address[1]}/atlas"
    gateways = [atlas, witnessed_atlas, hosted_atlas]
    for which, secret in (("witness", SERVING_SECRET), ("shared", APPLICATION_SECRET)):
        relay = Gateway(
            f"http://127.0.0.1:{provider.server_address[1]}",
            AiwatcherClient(service=f"e2e-gateway-{which}", base_url=BASE, token=secret),
            prompts=PromptRegistry(BASE, token=secret),
            upstream_token="provider-key",  # noqa: S106 — the stand-in provider's
            credential=secret,
            tools={
                "atlas": f"http://127.0.0.1:{atlas.server_address[1]}/atlas",
                "atlas-here": atlas_here,
            },
        )
        gateway = relay.server(port=0)
        threading.Thread(target=gateway.serve_forever, daemon=True).start()
        GATEWAYS[which] = f"http://127.0.0.1:{gateway.server_address[1]}"
        gateways.append(gateway)
    # An application holding the provider's key calls it without a gateway.
    GATEWAYS["provider"] = f"http://127.0.0.1:{provider.server_address[1]}"
    worker = Worker(
        BASE,
        WORKER_SECRET,
        queues=[QUEUE],
        tasks=[answer],
        name="e2e-generate-worker",
        poll_interval=0.1,
        telemetry=AiwatcherClient(service="e2e-generate", base_url=BASE, token=WORKER_SECRET),
    )
    serving = threading.Thread(target=worker.run, name="e2e-generate-worker", daemon=True)
    serving.start()
    try:
        dataset, cohort, card = shared()
        views = {
            which: declare(which, dataset, cohort, card) for which in ("baseline", "candidate")
        }
        views["declining"] = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-2",
            params={"decline": "Kenya"},
        )
        unheld = declare(
            "candidate",
            dataset,
            cohort,
            card,
            code=b"# a commit this worker was never built from\n",
        )
        started = {
            which: ok(
                *call("POST", f"/api/v1/evaluation-runs/{view['declaration']['id']}/start")[:2],
                f"starting the {which} run",
            )
            for which, view in views.items()
        }
        runs = {
            which: followed(accepted["execution"]["execution_id"])
            for which, accepted in started.items()
        }
        print("\nbaseline, candidate and a declining repetition:")

        states = {which: run["execution"]["state"]["state_type"] for which, run in runs.items()}
        steps = {
            which: [step["step_id"] for step in run["execution"]["steps"]]
            for which, run in runs.items()
        }
        check(
            1,
            "both runs complete through cases, generate and score",
            set(states.values()) == {"completed"}
            and all(
                sorted(ids) == ["cases", "generate", "score", "traces"] for ids in steps.values()
            ),
            {"states": states, "steps": steps},
        )
        check(
            2,
            "the worker is handed each case's question and nothing it expected",
            len(HANDED) >= len(CAPITALS)
            and all(set(row) == {"case_id", "input"} for row in HANDED),
            HANDED[:1],
        )

        results = {
            which: ok(
                *call(
                    "GET",
                    f"/api/v1/evaluation-results/{view['declaration']['run']['evaluation_id']}",
                )[:2],
                f"reading the {which} result",
            )
            for which, view in views.items()
        }
        exact = {which: result["metrics"].get("exact") for which, result in results.items()}
        contexts = {result["receipt"]["context_id"] for result in results.values()}
        check(
            3,
            "the candidate scores higher than the baseline, on one context",
            len(contexts) == 1 and (exact["candidate"] or 0) > (exact["baseline"] or 0),
            exact,
        )
        comparison = ok(
            *call(
                "GET",
                "/api/v1/evaluation-results/capitals-candidate/comparison?baseline=capitals-baseline",
            )[:2],
            "comparing the two",
        )
        deltas = {row["name"]: row.get("delta") for row in comparison.get("metrics", [])}
        check(
            4,
            "the comparison is comparable and names the rise",
            comparison.get("comparability") == "comparable" and (deltas.get("exact") or 0) > 0,
            {"comparability": comparison.get("comparability"), "deltas": deltas},
        )
        counts = results["declining"]["counts"]
        withheld = ok(
            *call(
                "GET",
                "/api/v1/evaluation-results/capitals-candidate-measurement-2/comparison"
                "?baseline=capitals-baseline",
            )[:2],
            "comparing the declining repetition",
        )
        check(
            5,
            "a declined case is unscored rather than a zero, and its comparison is withheld",
            counts["unscored"] == 1
            and counts["scored"] == len(CAPITALS) - 1
            and withheld.get("comparability") == "unverified"
            and all(row.get("delta") is None for row in withheld.get("metrics", [])),
            {"counts": counts, "reasons": withheld.get("reasons")},
        )

        manifest = results["candidate"]["manifest"]
        origin = manifest["origin"]
        _, page, _ = call(
            "GET",
            f"/api/v1/evaluation-results/capitals-candidate/cases?version={results['candidate']['receipt']['version']}",
        )
        traced = [
            case for case in (page or {}).get("cases", []) if case["measurement"].get("trace_id")
        ]
        check(
            6,
            "the result names its execution and step, and its cases the traces their runs had",
            origin.get("execution_id") == started["candidate"]["execution"]["execution_id"]
            and origin.get("step_id") == "score"
            and len(traced) == len(CAPITALS),
            {"origin": origin, "traced": len(traced)},
        )

        again = ok(
            *call(
                "POST",
                f"/api/v1/evaluation-runs/{views['candidate']['declaration']['id']}/start",
            )[:2],
            "starting the candidate again",
        )
        check(
            7,
            "starting a declaration again lands on the run it started",
            again["created"] is False
            and again["execution"]["execution_id"]
            == started["candidate"]["execution"]["execution_id"],
            again["created"],
        )

        refused = ok(
            *call("POST", f"/api/v1/evaluation-runs/{unheld['declaration']['id']}/start")[:2],
            "starting the unheld run",
        )
        stale = followed(refused["execution"]["execution_id"])
        told = json.dumps(stale["execution"])
        pinned = unheld["declaration"]["run"]["variant"]["code"]["digest"]
        held = hashlib.sha256(code_of("candidate")).hexdigest()
        published = call(
            "GET",
            f"/api/v1/evaluation-results/{unheld['declaration']['run']['evaluation_id']}",
        )[0]
        check(
            8,
            "a variant pinning code the worker does not hold is refused before an answer",
            stale["execution"]["state"]["state_type"] == "failed"
            and pinned in told
            and held in told
            and published == 404,
            {"state": stale["execution"]["state"]["state_type"], "result": published},
        )

        context_id = results["candidate"]["receipt"]["context_id"]
        viewed = ok(
            *call("GET", f"/api/v1/experiments/{context_id}?baseline=capitals-baseline")[:2],
            "reading the experiment",
        )
        rows = {row["evaluation_id"]: row for row in viewed["experiment"]["rows"]}
        candidate = rows.get("capitals-candidate", {})
        usage = candidate.get("usage") or {}
        priced = candidate.get("cost") or {}
        timed = {execution["workflow_run_id"] for execution in viewed["executions"]}
        # Six words in and one out per case, on the stand-in at 1 and 2 per million.
        expected_cost = (6 * len(CAPITALS) * 1.0 + len(CAPITALS) * 2.0) / 1e6
        check(
            9,
            "the experiment sets the variants side by side with what their answers took",
            {"capitals-baseline", "capitals-candidate"} <= set(rows)
            and (candidate.get("comparison") or {}).get("comparability") == "comparable"
            and (usage.get("latency_ms") or {}).get("cases") == len(CAPITALS)
            and (usage.get("output_tokens") or {}).get("cases") == len(CAPITALS)
            and started["candidate"]["execution"]["execution_id"] in timed
            and priced.get("priced_calls") == len(CAPITALS)
            and abs(priced.get("amount", 0) - expected_cost) < 1e-12
            and [price.get("as_of") for price in priced.get("prices", [])] == [PRICED_ON],
            {"rows": sorted(rows), "usage": usage, "cost": priced, "timed": len(timed)},
        )

        # The candidate, deployed: the same application serving somebody, each
        # run naming the variant the result was published as.
        variant_id = candidate.get("variant_id", "")
        production = AiwatcherClient(
            service="e2e-capitals",
            base_url=BASE,
            token=APPLICATION_SECRET,
            variant_id=variant_id,
        )
        served = 5
        for request in range(served):
            with (
                production.run(f"served-{request}") as traced,
                traced.agent("capitals") as agent,
                agent.llm(model="capitals-stand-in") as llm,
            ):
                time.sleep(0.02)
                llm.usage(prompt_tokens=6, completion_tokens=1)
        production.flush()
        TELEMETRY[0].flush()
        served_at = time.time()
        observed: dict[str, Any] = {}
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}")[:2],
                "reading what was observed",
            )
            observed = {row["variant_id"]: row for row in watched["observed"]}
            if (observed.get(variant_id) or {}).get("runs") == served:
                break
            time.sleep(0.3)
        seen = observed.get(variant_id) or {}
        baseline_seen = observed.get(rows.get("capitals-baseline", {}).get("variant_id", "")) or {}
        check(
            10,
            "what the candidate was observed serving stands beside it, its measurement left out",
            seen.get("runs") == served
            and seen.get("failed") == 0
            and (seen.get("duration_ms") or {}).get("count") == served
            and seen.get("input_tokens") == 6 * served
            and seen.get("measured_runs", 0) >= len(CAPITALS)
            and baseline_seen.get("runs") == 0
            and baseline_seen.get("measured_runs", 0) >= len(CAPITALS),
            {"candidate": seen, "baseline": baseline_seen},
        )

        traces = {which: result.get("traces") or {} for which, result in results.items()}
        check(
            11,
            "every generated answer's trace showed it made on the pinned prompt",
            traces["candidate"]
            == {
                "answers": len(CAPITALS),
                "named": len(CAPITALS),
                "seen": len(CAPITALS),
                "on_prompt": len(CAPITALS),
                # Nobody but the application saw these calls.
                "witnessed_prompt": 0,
                "witnessed_answer": 0,
                "witnessed_input": 0,
                "witnessed_exchange": 0,
                # Calls asked elsewhere looked for from the measurement's start.
                "asked_since_seconds": 0,
            }
            and traces["declining"].get("on_prompt") == len(CAPITALS) - 1,
            traces,
        )

        pins = {
            which: views[which]["declaration"]["run"]["variant"]["prompt"]["version"]
            for which in ("baseline", "candidate")
        }
        rendering = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-3",
            params={"render_version": pins["baseline"]},
        )
        misrendered = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{rendering['declaration']['id']}/start",
                )[:2],
                "starting the run that renders another prompt",
            )["execution"]["execution_id"]
        )
        told = json.dumps(misrendered["execution"])
        check(
            12,
            "answers whose traces show another version of the pinned prompt are never scored",
            misrendered["execution"]["state"]["state_type"] == "failed"
            and [
                step["step_id"]
                for step in misrendered["execution"]["steps"]
                if '"failed"' in json.dumps(step)
            ]
            == ["traces"]
            and pins["baseline"] in told
            and pins["candidate"] in told
            and call(
                "GET",
                f"/api/v1/evaluation-results/{rendering['declaration']['run']['evaluation_id']}",
            )[0]
            == 404,
            {
                "state": misrendered["execution"]["state"]["state_type"],
                "failed_at": [
                    step["step_id"]
                    for step in misrendered["execution"]["steps"]
                    if '"failed"' in json.dumps(step)
                ],
            },
        )

        # A variant also pinning a model nobody here registered and a workflow:
        # the application executes the workflow, and a model server reports
        # the call it served under its own credential.
        flowing = declare("candidate", dataset, cohort, card, served=True, suffix="-served")
        flowed = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{flowing['declaration']['id']}/start",
                )[:2],
                "starting the run on a served model and a pinned workflow",
            )["execution"]["execution_id"]
        )
        evaluation = flowing["declaration"]["run"]["evaluation_id"]
        witnessed = (call("GET", f"/api/v1/evaluation-results/{evaluation}")[1] or {}).get(
            "traces"
        ) or {}
        spans = call("GET", "/api/v1/runs?limit=200")[1] or {}
        publishers = {
            run.get("published_by")
            for run in spans.get("runs", [])
            if run["run_id"].startswith(("generate-capitals-candidate-served", "gateway-"))
        }
        check(
            13,
            "every answer is seen executing the pinned workflow on the pinned model, and the "
            "gateway's own run under the witness credential witnesses its model, its prompt, "
            "its answer and its question",
            flowed["execution"]["state"]["state_type"] == "completed"
            and witnessed
            == {
                "answers": len(CAPITALS),
                "named": len(CAPITALS),
                "seen": len(CAPITALS),
                "on_prompt": len(CAPITALS),
                "on_model": len(CAPITALS),
                "on_workflow": len(CAPITALS),
                "witnessed_model": len(CAPITALS),
                "witnessed_prompt": len(CAPITALS),
                "witnessed_answer": len(CAPITALS),
                "witnessed_input": len(CAPITALS),
                "witnessed_exchange": len(CAPITALS),
                "asked_since_seconds": 0,
                "witnesses": ["serving"],
            }
            and publishers == {"application", "serving"},
            {
                "state": flowed["execution"]["state"]["state_type"],
                "traces": witnessed,
                "published_by": sorted(str(name) for name in publishers),
            },
        )

        straying = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-4",
            params={"stray": True},
            served=True,
            suffix="-served",
        )
        strayed = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{straying['declaration']['id']}/start",
                )[:2],
                "starting the run whose application steps off the pinned workflow",
            )["execution"]["execution_id"]
        )
        told = json.dumps(strayed["execution"])
        reordering = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-5",
            params={"reorder": True},
            served=True,
            suffix="-served",
        )
        reordered = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{reordering['declaration']['id']}/start",
                )[:2],
                "starting the run whose application answers before it retrieves",
            )["execution"]["execution_id"]
        )
        doubling = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-8",
            params={"twice": True},
            served=True,
            suffix="-served",
        )
        doubled = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{doubling['declaration']['id']}/start",
                )[:2],
                "starting the run whose application answers twice for one retrieval",
            )["execution"]["execution_id"]
        )
        check(
            14,
            "answers whose run stepped off the pinned workflow, out of its order, or through a "
            "node more often than it leads are never scored",
            strayed["execution"]["state"]["state_type"] == "failed"
            and [
                step["step_id"]
                for step in strayed["execution"]["steps"]
                if '"failed"' in json.dumps(step)
            ]
            == ["traces"]
            and "improvise" in told
            and call(
                "GET",
                f"/api/v1/evaluation-results/{straying['declaration']['run']['evaluation_id']}",
            )[0]
            == 404
            and reordered["execution"]["state"]["state_type"] == "failed"
            and "started answer before retrieve had completed" in json.dumps(reordered["execution"])
            and doubled["execution"]["state"]["state_type"] == "failed"
            and "started answer again with no completion of retrieve since"
            in json.dumps(doubled["execution"]),
            {
                "stray": strayed["execution"]["state"]["state_type"],
                "reordered": reordered["execution"]["state"]["state_type"],
                "doubled": doubled["execution"]["state"]["state_type"],
            },
        )

        # The same runs, through a window: once the periods they ended in are
        # written, they are counted from those — and only from those.
        windowed: dict[str, Any] = {}
        # A period closes once the log's clock has passed it by a second, and
        # every run after the served ones moves that clock on.
        time.sleep(max(0.0, served_at + 12 - time.time()))
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}?window_seconds=3600")[:2],
                "reading what was observed over a window",
            )
            windowed = next(
                (row for row in watched["observed"] if row["variant_id"] == variant_id),
                {},
            )
            if windowed.get("runs_from_periods") == served:
                break
            time.sleep(1)
        cost = windowed.get("cost") or {}
        check(
            15,
            "observations over a window are the period fold's, counted once from where they "
            "began, each call timed and priced at a dated price",
            windowed.get("runs") == served
            and windowed.get("runs_from_periods") == served
            and windowed.get("window_before_observations") is True
            and bool(windowed.get("counted_from"))
            and (windowed.get("duration_ms") or {}).get("bucketed") is True
            and (windowed.get("call_ms") or {}).get("count") == served
            and cost.get("priced_calls") == served
            and abs(cost.get("amount", 0) - (6 * served * 1.0 + served * 2.0) / 1e6) < 1e-12
            and [price.get("as_of") for price in cost.get("prices", [])] == [PRICED_ON],
            {
                key: windowed.get(key)
                for key in (
                    "runs",
                    "runs_from_periods",
                    "periods",
                    "incomplete_periods",
                    "counted_from",
                    "window_before_observations",
                    "duration_ms",
                    "call_ms",
                    "cost",
                )
            },
        )

        # A gateway holding the application's own token.
        sharing = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-6",
            params={"shared": True},
            served=True,
            suffix="-served",
        )
        shared_run = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{sharing['declaration']['id']}/start",
                )[:2],
                "starting the run whose gateway holds the application's token",
            )["execution"]["execution_id"]
        )
        sharing_id = sharing["declaration"]["run"]["evaluation_id"]
        shared_traces = (call("GET", f"/api/v1/evaluation-results/{sharing_id}")[1] or {}).get(
            "traces"
        ) or {}
        gated = (
            call(
                "POST",
                f"/api/v1/evaluation-results/{sharing_id}/gate",
                {"baseline": evaluation, "policy": {"require_witness": True}},
            )[1]
            or {}
        )
        check(
            16,
            "a gateway holding the application's own token witnesses nothing, and a gate "
            "requiring a witness says so",
            shared_run["execution"]["state"]["state_type"] == "completed"
            and shared_traces.get("self_witnessed") == len(CAPITALS)
            and shared_traces.get("witnessed_model") == 0
            and gated.get("verdict") == "incomplete"
            and any("a token of its own" in reason for reason in gated.get("reasons", [])),
            {"traces": shared_traces, "gate": gated.get("reasons")},
        )

        # An application asking the witness about somewhere else, and answering
        # from a call it made around the gateway.
        bypassing = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-9",
            params={"around": True},
            served=True,
            suffix="-served",
        )
        bypassed = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{bypassing['declaration']['id']}/start",
                )[:2],
                "starting the run whose application answers around the gateway",
            )["execution"]["execution_id"]
        )
        bypassing_id = bypassing["declaration"]["run"]["evaluation_id"]
        around = (call("GET", f"/api/v1/evaluation-results/{bypassing_id}")[1] or {}).get(
            "traces"
        ) or {}
        held = (
            call(
                "POST",
                f"/api/v1/evaluation-results/{bypassing_id}/gate",
                {
                    "baseline": evaluation,
                    "policy": {
                        "require_witness": True,
                        "require_witnessed_answer": True,
                    },
                },
            )[1]
            or {}
        )
        check(
            17,
            "an answer made around the gateway has its model and prompt witnessed and neither "
            "its answer nor its question, and a gate requiring witnessed answers says so",
            bypassed["execution"]["state"]["state_type"] == "completed"
            and around.get("witnessed_model") == len(CAPITALS)
            and around.get("witnessed_prompt") == len(CAPITALS)
            and around.get("witnessed_answer") == 0
            and around.get("witnessed_input") == 0
            and around.get("witnessed_exchange") == 0
            and held.get("verdict") == "incomplete"
            and any("made around the gateway" in reason for reason in held.get("reasons", [])),
            {"traces": around, "gate": held.get("reasons")},
        )

        # An application telling the witnessed model what to say, one whose
        # model reasons before it answers, and one handing it a value it made.
        told_runs: dict[str, dict[str, Any]] = {}
        for which, measurement in (
            ("repeat", "measurement-10"),
            ("explain", "measurement-11"),
            ("hint", "measurement-12"),
        ):
            declared = declare(
                "candidate",
                dataset,
                cohort,
                card,
                repetition=measurement,
                params={which: True},
                served=True,
                suffix="-served",
            )
            ran = followed(
                ok(
                    *call(
                        "POST",
                        f"/api/v1/evaluation-runs/{declared['declaration']['id']}/start",
                    )[:2],
                    f"starting the run whose application does {which}",
                )["execution"]["execution_id"]
            )
            result_id = declared["declaration"]["run"]["evaluation_id"]
            told_runs[which] = {
                "state": ran["execution"]["state"]["state_type"],
                "traces": (call("GET", f"/api/v1/evaluation-results/{result_id}")[1] or {}).get(
                    "traces"
                )
                or {},
                "gate": (
                    call(
                        "POST",
                        f"/api/v1/evaluation-results/{result_id}/gate",
                        {
                            "baseline": evaluation,
                            "policy": {"require_witnessed_answer": True},
                        },
                    )[1]
                    or {}
                ),
            }
        repeated = told_runs["repeat"]
        check(
            18,
            "an answer the application told the witnessed model to say is the reply and no "
            "exchange, and a gate requiring witnessed answers says why",
            repeated["state"] == "completed"
            and repeated["traces"].get("witnessed_answer") == len(CAPITALS)
            and repeated["traces"].get("witnessed_input") == len(CAPITALS)
            and repeated["traces"].get("witnessed_exchange") == 0
            and repeated["gate"].get("verdict") == "incomplete"
            and any(
                "nothing but the pinned prompt" in reason
                for reason in repeated["gate"].get("reasons", [])
            ),
            {"traces": repeated["traces"], "gate": repeated["gate"].get("reasons")},
        )
        explained = told_runs["explain"]
        check(
            19,
            "an answer taken out of a reasoned reply the way the application said is witnessed "
            "as that reply's, exchange and all",
            explained["state"] == "completed"
            and explained["traces"].get("witnessed_exchange") == len(CAPITALS)
            and explained["gate"].get("verdict") in {"pass", "regression"}
            and not any(
                "witnessed as the reply" in reason
                for reason in explained["gate"].get("reasons", [])
            ),
            {"traces": explained["traces"], "gate": explained["gate"].get("reasons")},
        )
        hinted = told_runs["hint"]
        check(
            20,
            "an answer to a request rendered with a value the application made beside the "
            "case's own is the reply and no exchange, and a gate requiring witnessed answers "
            "says why",
            hinted["state"] == "completed"
            and hinted["traces"].get("witnessed_answer") == len(CAPITALS)
            and hinted["traces"].get("witnessed_input") == len(CAPITALS)
            and hinted["traces"].get("witnessed_exchange") == 0
            and hinted["gate"].get("verdict") == "incomplete"
            and any(
                "a value the application made" in reason
                for reason in hinted["gate"].get("reasons", [])
            ),
            {"traces": hinted["traces"], "gate": hinted["gate"].get("reasons")},
        )

        # A request naming the pinned prompt with other words in it.
        drifting = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-7",
            params={"drift": True},
            served=True,
            suffix="-served",
        )
        drifted = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{drifting['declaration']['id']}/start",
                )[:2],
                "starting the run whose requests do not hold the pinned prompt",
            )["execution"]["execution_id"]
        )
        check(
            21,
            "a request whose text does not hold the pinned prompt is refused on the gateway's word",
            drifted["execution"]["state"]["state_type"] == "failed"
            and "does not hold that version's template" in json.dumps(drifted["execution"])
            and call(
                "GET",
                f"/api/v1/evaluation-results/{drifting['declaration']['run']['evaluation_id']}",
            )[0]
            == 404,
            {"state": drifted["execution"]["state"]["state_type"]},
        )

        # Values accounted for beyond the case's input and a reply: a country
        # cut out of the question, one out of a tool's answer, a label's word the
        # variant pins or does not, and an answer made of two replies.
        accounted: dict[str, dict[str, Any]] = {}
        for which, params, measurement in (
            ("cut", {"cut": True}, "measurement-13"),
            ("cut", {"atlas": True}, "measurement-14"),
            ("labelled", {"label": True}, "measurement-15"),
            ("labelled-loose", {"label": True}, "measurement-16"),
            ("composed", {"composed": True}, "measurement-17"),
            ("joined", {"joined": True}, "measurement-18"),
            ("chosen", {"chosen": True}, "measurement-19"),
            ("chosen-loose", {"chosen": True}, "measurement-20"),
            ("cut", {"witnessed_atlas": True}, "measurement-21"),
            ("judged", {"judged": True}, "measurement-22"),
            ("judged", {"judged_against": True}, "measurement-23"),
            ("candidate", {"peek": True}, "measurement-24"),
            ("cut", {"hosted_atlas": True}, "measurement-25"),
            ("cut", {"gateway_atlas": True}, "measurement-26"),
        ):
            declared = declare(
                which,
                dataset,
                cohort,
                card,
                repetition=measurement,
                params=params,
                served=True,
                suffix="-served",
            )
            ran = followed(
                ok(
                    *call(
                        "POST",
                        f"/api/v1/evaluation-runs/{declared['declaration']['id']}/start",
                    )[:2],
                    f"starting the {which} run with {sorted(params)}",
                )["execution"]["execution_id"]
            )
            result_id = declared["declaration"]["run"]["evaluation_id"]
            accounted[f"{which}:{next(iter(params))}"] = {
                "state": ran["execution"]["state"]["state_type"],
                "traces": (call("GET", f"/api/v1/evaluation-results/{result_id}")[1] or {}).get(
                    "traces"
                )
                or {},
                "gate": (
                    call(
                        "POST",
                        f"/api/v1/evaluation-results/{result_id}/gate",
                        {
                            "baseline": evaluation,
                            "policy": {"require_witnessed_answer": True},
                        },
                    )[1]
                    or {}
                ),
            }

        def exchanged(key: str) -> int | None:
            return accounted[key]["traces"].get("witnessed_exchange")

        check(
            22,
            "a country the application cut out of the question in the steps it named is an "
            "exchange on every answer",
            accounted["cut:cut"]["state"] == "completed" and exchanged("cut:cut") == len(CAPITALS),
            accounted["cut:cut"]["traces"],
        )
        check(
            23,
            "a country taken out of the answer of an atlas the gateway relayed, for the case's "
            "own question, is an exchange on every answer",
            accounted["cut:atlas"]["state"] == "completed"
            and exchanged("cut:atlas") == len(CAPITALS),
            accounted["cut:atlas"]["traces"],
        )
        loose = accounted["labelled-loose:label"]
        check(
            24,
            "a label's capital is an exchange where the variant pins the way it is taken, and "
            "neither the reply nor an exchange where it does not",
            accounted["labelled:label"]["state"] == "completed"
            and exchanged("labelled:label") == len(CAPITALS)
            and loose["state"] == "completed"
            and loose["traces"].get("witnessed_answer") == 0
            and exchanged("labelled-loose:label") == 0
            and loose["gate"].get("verdict") == "incomplete"
            and any(
                "a label's word the variant does not pin" in reason
                for reason in loose["gate"].get("reasons", [])
            ),
            {
                "pinned": accounted["labelled:label"]["traces"],
                "loose": loose["traces"],
                "gate": loose["gate"].get("reasons"),
            },
        )
        check(
            25,
            "an answer made of two witnessed replies, in the pinned schema's shape, is an "
            "exchange part by part",
            accounted["composed:composed"]["state"] == "completed"
            and exchanged("composed:composed") == len(CAPITALS),
            accounted["composed:composed"]["traces"],
        )

        check(
            26,
            "two witnessed replies joined in the words the variant pins are an exchange on every "
            "answer",
            accounted["joined:joined"]["state"] == "completed"
            and exchanged("joined:joined") == len(CAPITALS),
            accounted["joined:joined"]["traces"],
        )
        loose = accounted["chosen-loose:chosen"]
        check(
            27,
            "an answer most of three witnessed replies gave is an exchange where the variant pins "
            "choosing by the most of three, and a choice the application made where it does not",
            accounted["chosen:chosen"]["state"] == "completed"
            and exchanged("chosen:chosen") == len(CAPITALS)
            and loose["state"] == "completed"
            and loose["traces"].get("witnessed_answer") == len(CAPITALS)
            and exchanged("chosen-loose:chosen") == 0
            and loose["traces"].get("chosen") == len(CAPITALS)
            and loose["gate"].get("verdict") == "incomplete"
            and any(
                "chosen among replies" in reason for reason in loose["gate"].get("reasons", [])
            ),
            {
                "pinned": accounted["chosen:chosen"]["traces"],
                "loose": loose["traces"],
                "gate": loose["gate"].get("reasons"),
            },
        )
        check(
            28,
            "a country from an atlas the application called directly, witnessed on the atlas's "
            "own host under the witness's credential, is an exchange on every answer",
            accounted["cut:witnessed_atlas"]["state"] == "completed"
            and exchanged("cut:witnessed_atlas") == len(CAPITALS),
            accounted["cut:witnessed_atlas"]["traces"],
        )

        against = accounted["judged:judged_against"]
        check(
            29,
            "an answer a witnessed judging call named, on the judging prompt the variant pins and "
            "taken out the way it pins, is an exchange on every answer; the reply it did not name "
            "is a choice the application made",
            accounted["judged:judged"]["state"] == "completed"
            and exchanged("judged:judged") == len(CAPITALS)
            and against["state"] == "completed"
            and exchanged("judged:judged_against") == 0
            and against["traces"].get("chosen") == len(CAPITALS),
            {
                "named": accounted["judged:judged"]["traces"],
                "against": against["traces"],
            },
        )
        peeked = accounted["candidate:peek"]
        check(
            30,
            "an application that first asks each question on the pinned prompt through the "
            "witness in a run of no measurement's, saying nothing of what it rendered, has no "
            "exchange, and a gate says each case was asked elsewhere",
            peeked["state"] == "completed"
            and exchanged("candidate:peek") == 0
            and peeked["traces"].get("asked_elsewhere") == len(CAPITALS)
            and peeked["gate"].get("verdict") == "incomplete"
            and any(
                "asked on the pinned prompt in other runs" in reason
                for reason in peeked["gate"].get("reasons", [])
            ),
            {"traces": peeked["traces"], "gate": peeked["gate"].get("reasons")},
        )
        check(
            31,
            "a country from an atlas whose host publishes under a token of its own, digesting "
            "under the gateway's witness key as the deployment says it may, is an exchange on "
            "every answer",
            accounted["cut:hosted_atlas"]["state"] == "completed"
            and exchanged("cut:hosted_atlas") == len(CAPITALS),
            accounted["cut:hosted_atlas"]["traces"],
        )
        check(
            32,
            "a country from an atlas the gateway answers itself, in its own process, is an "
            "exchange on every answer",
            accounted["cut:gateway_atlas"]["state"] == "completed"
            and exchanged("cut:gateway_atlas") == len(CAPITALS),
            accounted["cut:gateway_atlas"]["traces"],
        )

        def measure(
            which: str,
            params: dict[str, Any],
            measurement: str,
            *,
            served: bool = True,
            settings: dict[str, Any] | None = None,
            before: Any = None,
            policy: dict[str, Any] | None = None,
        ) -> dict[str, Any]:
            """Declare, do `before` with the declaration, run, and read the result and a gate."""
            declared = declare(
                which,
                dataset,
                cohort,
                card,
                repetition=measurement,
                params=params,
                served=served,
                suffix="-served" if served else "",
                settings=settings,
            )
            if before is not None:
                before(declared)
            ran = followed(
                ok(
                    *call(
                        "POST",
                        f"/api/v1/evaluation-runs/{declared['declaration']['id']}/start",
                    )[:2],
                    f"starting the {which} run with {sorted(params)}",
                )["execution"]["execution_id"]
            )
            result_id = declared["declaration"]["run"]["evaluation_id"]
            status, result, _ = call("GET", f"/api/v1/evaluation-results/{result_id}")
            return {
                "state": ran["execution"]["state"]["state_type"],
                "execution": ran["execution"],
                "published": status,
                "traces": (result or {}).get("traces") or {},
                "gate": (
                    call(
                        "POST",
                        f"/api/v1/evaluation-results/{result_id}/gate",
                        {
                            "baseline": evaluation,
                            "policy": policy or {"require_witnessed_answer": True},
                        },
                    )[1]
                    or {}
                )
                if status == 200
                else {},
            }

        def peek(
            declared: dict[str, Any],
            *,
            prompt: tuple[str, str] | None = None,
            spelled: Any = None,
            caller: str | None = None,
        ) -> None:
            """Ask every case's question through the witness, as no measurement's run —
            on the pinned prompt unless another is named, spelled as given."""
            name, version = prompt or (
                PROMPT,
                declared["declaration"]["run"]["variant"]["prompt"]["version"],
            )
            with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
                text = prompts.get_version(name, version)
            for country, _ in CAPITALS:
                question = f"What is the capital of {country}?"
                asked = spelled(question) if spelled else question
                through_gateway(
                    "witness",
                    {
                        PROMPT_HEADER: f"{name}@{version}",
                        **({CALLER_RUN_HEADER: f"{caller}-{country}"} if caller else {}),
                    },
                    text.render(question=asked),
                    asked,
                )

        # Asked before a restart the read model will not hold: declared and
        # peeked at now, run once many runs later and the server restarted.
        restarting = declare(
            "asked-restart",
            dataset,
            cohort,
            card,
            repetition="measurement-40",
            served=True,
            suffix="-served",
            settings={"asked_since_seconds": 3600},
        )
        peek(restarting, caller="peek")

        # A judge shown the candidates in an order the application does not choose.
        judged_runs = {
            "gateway": measure(
                "judged-witnessed",
                {"judge_order": "gateway", "judge_mode": "judge-right"},
                "measurement-27",
            ),
            "descending": measure(
                "judged-witnessed",
                {"judge_order": "descending", "judge_mode": "judge-right"},
                "measurement-28",
            ),
            "both": measure(
                "judged-both",
                {"judge_order": "both", "judge_mode": "judge-right"},
                "measurement-29",
            ),
            "both-apart": measure(
                "judged-both",
                {"judge_order": "both", "judge_mode": "judge"},
                "measurement-30",
            ),
        }
        placed = judged_runs["gateway"]
        misplaced = judged_runs["descending"]
        check(
            36,
            "an answer a judge picked among candidates the gateway placed in the witnessed order "
            "is an exchange on every answer, the application reading where they went from the "
            "gateway's reply",
            placed["state"] == "completed"
            and placed["traces"].get("witnessed_exchange") == len(CAPITALS)
            and not placed["traces"].get("judged_unordered"),
            placed["traces"],
        )
        check(
            37,
            "candidates the application placed against the witnessed order are no exchange, and "
            "the gate names why",
            misplaced["state"] == "completed"
            and misplaced["traces"].get("witnessed_exchange") == 0
            and misplaced["traces"].get("chosen") == len(CAPITALS)
            and any(
                "not the witnessed order" in reason
                for reason in misplaced["gate"].get("reasons", [])
            ),
            {"traces": misplaced["traces"], "gate": misplaced["gate"].get("reasons")},
        )
        check(
            38,
            "a judge asked both ways, the second time reversed, is an exchange where both name "
            "the answer, and none where they name different replies",
            judged_runs["both"]["state"] == "completed"
            and judged_runs["both"]["traces"].get("witnessed_exchange") == len(CAPITALS)
            and judged_runs["both-apart"]["state"] == "completed"
            and judged_runs["both-apart"]["traces"].get("witnessed_exchange") == 0
            and any(
                "named different replies when asked both ways" in reason
                for reason in judged_runs["both-apart"]["gate"].get("reasons", [])
            ),
            {
                "both": judged_runs["both"]["traces"],
                "apart": judged_runs["both-apart"]["traces"],
                "gate": judged_runs["both-apart"]["gate"].get("reasons"),
            },
        )

        # Questions asked before the measurement, in other words, and on another prompt.
        before_start = measure(
            "asked-before",
            {},
            "measurement-31",
            settings={"asked_since_seconds": 600},
            before=peek,
        )
        unread = measure("asked-unread", {}, "measurement-32", before=peek)
        check(
            39,
            "a case asked before the measurement started, by its question alone — a part of the "
            "case's input — is asked elsewhere where the run reads from before its start, and "
            "not where it reads from its start",
            before_start["state"] == "completed"
            and before_start["traces"].get("witnessed_exchange") == 0
            and before_start["traces"].get("asked_elsewhere") == len(CAPITALS)
            and unread["state"] == "completed"
            and unread["traces"].get("witnessed_exchange") == len(CAPITALS),
            {"from_before": before_start["traces"], "from_start": unread["traces"]},
        )
        normal = measure(
            "asked-normal",
            {},
            "measurement-33",
            settings={"asked_since_seconds": 600},
            before=lambda declared: peek(
                declared,
                spelled=lambda question: "  " + question.upper().replace(" ", "   "),
            ),
        )
        check(
            40,
            "a case asked in another case and spacing is asked elsewhere all the same",
            normal["state"] == "completed"
            and normal["traces"].get("witnessed_exchange") == 0
            and normal["traces"].get("asked_elsewhere") == len(CAPITALS),
            normal["traces"],
        )
        with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
            chat = prompts.publish(CHAT_PROMPT, CHAT, author="e2e").version_id
        unpinned = measure(
            "asked-unpinned",
            {},
            "measurement-34",
            settings={"asked_since_seconds": 600},
            before=lambda declared: peek(declared, prompt=(CHAT_PROMPT, chat)),
        )
        strict = (
            call(
                "POST",
                "/api/v1/evaluation-results/capitals-asked-unpinned-measurement-34-served/gate",
                {
                    "baseline": evaluation,
                    "policy": {
                        "require_witnessed_answer": True,
                        "asked_elsewhere_unpinned_denies": True,
                    },
                },
            )[1]
            or {}
        )
        check(
            41,
            "a case asked on another prompt is counted apart and denies no exchange — unless a "
            "gate's policy says it does",
            unpinned["state"] == "completed"
            and unpinned["traces"].get("witnessed_exchange") == len(CAPITALS)
            and unpinned["traces"].get("asked_elsewhere_unpinned") == len(CAPITALS)
            and not unpinned["traces"].get("asked_elsewhere")
            and not any(
                "another prompt" in reason for reason in unpinned["gate"].get("reasons", [])
            )
            and strict.get("verdict") == "incomplete"
            and any("another prompt or model" in reason for reason in strict.get("reasons", [])),
            {"traces": unpinned["traces"], "strict": strict.get("reasons")},
        )

        # A tool in the application's process, and one the gateway answers by pinned code.
        in_app = measure("cut-in-app", {"atlas_in_app": True}, "measurement-35")
        pinned_tool = measure("cut-pinned", {"gateway_atlas": True}, "measurement-36")
        mispinned = measure("cut-mispinned", {"gateway_atlas": True}, "measurement-37")
        told = json.dumps(mispinned["execution"])
        check(
            42,
            "a country an atlas computed in the application's own process is no exchange, and "
            "the trace and the gate name the atlas as where it may have come from",
            in_app["state"] == "completed"
            and in_app["traces"].get("witnessed_exchange") == 0
            and in_app["traces"].get("unaccounted_tools") == ["atlas"]
            and any(
                "called atlas with no witness" in reason
                for reason in in_app["gate"].get("reasons", [])
            ),
            {"traces": in_app["traces"], "gate": in_app["gate"].get("reasons")},
        )
        check(
            43,
            "an atlas the gateway answers with the code the variant pins is an exchange on every "
            "answer, and answered with other code the answers are refused naming both digests",
            pinned_tool["state"] == "completed"
            and pinned_tool["traces"].get("witnessed_exchange") == len(CAPITALS)
            and mispinned["state"] == "failed"
            and "the tool atlas-here with code" in told
            and ATLAS_CODE in told
            and hashlib.sha256(b"# another atlas\n").hexdigest() in told
            and mispinned["published"] == 404,
            {"pinned": pinned_tool["traces"], "mispinned": mispinned["state"]},
        )
        url_pinned = measure("cut-url-pinned", {"atlas": True}, "measurement-42")
        hosted_pinned = measure("cut-hosted-pinned", {"hosted_atlas": True}, "measurement-43")
        url_mispinned = measure("cut-url-mispinned", {"atlas": True}, "measurement-44")
        url_told = json.dumps(url_mispinned["execution"])
        check(
            48,
            "an atlas at the URL the deployment named, naming its code in its reply, and one on a "
            "host naming it where it witnesses, are held to the code the variant pins",
            url_pinned["state"] == "completed"
            and url_pinned["traces"].get("witnessed_exchange") == len(CAPITALS)
            and hosted_pinned["state"] == "completed"
            and hosted_pinned["traces"].get("witnessed_exchange") == len(CAPITALS)
            and url_mispinned["state"] == "failed"
            and "the tool atlas with code" in url_told
            and ATLAS_CODE in url_told
            and url_mispinned["published"] == 404,
            {
                "url": url_pinned["traces"],
                "hosted": hosted_pinned["traces"],
                "mispinned": url_mispinned["state"],
            },
        )

        def gated_on_lookback(result_id: str) -> dict[str, Any]:
            return (
                call(
                    "POST",
                    f"/api/v1/evaluation-results/{result_id}/gate",
                    {
                        "baseline": evaluation,
                        "policy": {"require_witnessed_answer": True, "asked_since_seconds": 600},
                    },
                )[1]
                or {}
            )

        short = gated_on_lookback("capitals-asked-unread-measurement-32-served")
        long_enough = gated_on_lookback("capitals-asked-unpinned-measurement-34-served")
        check(
            49,
            "a result that read calls asked elsewhere from its start is held incomplete by a gate "
            "asking for ten minutes before it, and one that read from ten minutes before is not",
            short.get("verdict") == "incomplete"
            and any("from 0 s before" in reason for reason in short.get("reasons", []))
            and not any(
                "asked elsewhere from" in reason for reason in long_enough.get("reasons", [])
            ),
            {"short": short.get("reasons"), "long_enough": long_enough.get("reasons")},
        )

        # A measurement's runs, counted: one lost in transport, one made up, and a retry.
        lost = measure("counted", {"lose_run": True}, "measurement-38", served=False)
        invented = measure("counted", {"invent": True}, "measurement-39", served=False)
        retried = measure("counted", {"fail_first": True}, "measurement-41", served=False)
        attempts = [
            step.get("attempt")
            for step in retried["execution"]["steps"]
            if step["step_id"] == "generate"
        ]
        check(
            44,
            "an answer whose run its client lost in transport is said to be lost, one naming a "
            "run nobody opened is said to be unknown, and a retried attempt leaves no gap",
            lost["state"] == "completed"
            and lost["traces"].get("seen") == len(CAPITALS) - 1
            and lost["traces"].get("lost_in_transport") == 1
            and not lost["traces"].get("unknown_runs")
            and invented["state"] == "completed"
            and invented["traces"].get("unknown_runs") == 1
            and not invented["traces"].get("lost_in_transport")
            and retried["state"] == "completed"
            and retried["traces"].get("seen") == len(CAPITALS)
            and not retried["traces"].get("lost_in_transport")
            and not retried["traces"].get("unknown_runs"),
            {
                "lost": lost["traces"],
                "invented": invented["traces"],
                "retried": retried["traces"],
                "attempts": attempts,
            },
        )

        # A restart with a period open: the fold's saved state carries it.
        before = AiwatcherClient(
            service="e2e-capitals",
            base_url=BASE,
            token=APPLICATION_SECRET,
            variant_id=variant_id,
        )
        for request in range(3):
            with (
                before.run(f"before-restart-{request}") as traced,
                traced.agent("capitals") as agent,
                agent.llm(model="capitals-stand-in") as llm,
            ):
                llm.usage(prompt_tokens=6, completion_tokens=1)
        before.flush()
        worker.stop()
        serving.join(timeout=5)
        server.terminate()
        server.wait(timeout=30)
        # On the same port, so the gateways keep reaching it — and with a read
        # model holding the last forty runs, none of them the calls asked
        # before it went down.
        server = serve(
            home / "server",
            port=int(BASE.rsplit(":", 1)[1]),
            extra={"AIWATCHER_MAX_RUNS": "40"},
        )
        dropping = Dropping(HttpTransport(BASE, token=APPLICATION_SECRET), "after-restart-lost")
        after = AiwatcherClient(service="e2e-capitals", transport=dropping, variant_id=variant_id)
        # Past the period the three ended in, so these close it — with a run
        # between them whose every event the transport dropped.
        time.sleep(7)
        for request in ("0", "lost", "1"):
            with (
                after.run(f"after-restart-{request}") as traced,
                traced.agent("capitals") as agent,
                agent.llm(model="capitals-stand-in") as llm,
            ):
                llm.usage(prompt_tokens=6, completion_tokens=1)
        after.flush()
        restarted: dict[str, Any] = {}
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}?window_seconds=3600")[:2],
                "reading what was observed after the restart",
            )
            restarted = next(
                (row for row in watched["observed"] if row["variant_id"] == variant_id),
                {},
            )
            if (
                restarted.get("runs_from_periods") == served + 3
                and restarted.get("runs") == served + 5
                and restarted.get("lost_runs") == 1
            ):
                break
            time.sleep(1)
        pages = sorted(
            (home / "server" / ".data" / "prompts" / "variant-observations" / "journal").rglob(
                "*.json"
            )
        )
        paged = [json.loads(page.read_text()) for page in pages]
        kept_words = any(
            "What is the capital" in json.dumps(event) for page in paged for event in page["events"]
        )
        check(
            33,
            "a restart that replays the log over the fold's saved state counts every run once, "
            "and the journal paged what the fold reads without a word said in a run",
            restarted.get("runs") == served + 5
            and restarted.get("runs_from_periods") == served + 3
            and any(page["events"] for page in paged)
            and not kept_words,
            {
                **{key: restarted.get(key) for key in ("runs", "runs_from_periods", "periods")},
                "journal_pages": len(pages),
            },
        )
        check(
            34,
            "a run whose every event its transport dropped is counted as lost, by the gap in its "
            "client's count of the runs it opened, and in no other figure",
            restarted.get("lost_runs") == 1 and restarted.get("runs") == served + 5,
            {key: restarted.get(key) for key in ("runs", "lost_runs", "lost_events")},
        )

        # A client new since the restart, whose very first run the transport
        # drops: the fold was reading when its count began — more than the
        # minute a client's clock is allowed to run ahead after it started.
        time.sleep(max(0.0, reading_since + 65 - time.time()))
        fresh = AiwatcherClient(
            service="e2e-capitals",
            transport=Dropping(HttpTransport(BASE, token=APPLICATION_SECRET), "fresh-lost"),
            variant_id=variant_id,
        )
        for request in ("lost", "1"):
            with (
                fresh.run(f"fresh-{request}") as traced,
                traced.agent("capitals") as agent,
                agent.llm(model="capitals-stand-in") as llm,
            ):
                llm.usage(prompt_tokens=6, completion_tokens=1)
        fresh.flush()
        renewed: dict[str, Any] = {}
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}?window_seconds=3600")[:2],
                "reading what was observed of a client new since the restart",
            )
            renewed = next(
                (row for row in watched["observed"] if row["variant_id"] == variant_id),
                {},
            )
            if renewed.get("lost_runs") == 2 and renewed.get("runs") == served + 6:
                break
            time.sleep(1)
        check(
            35,
            "the first run of a client whose count began while the fold was reading, dropped "
            "whole, is counted as lost once the client's second run arrives",
            renewed.get("lost_runs") == 2 and renewed.get("runs") == served + 6,
            {key: renewed.get(key) for key in ("runs", "lost_runs")},
        )

        # After the restart: the questions asked long before it, from the index.
        worker = Worker(
            BASE,
            WORKER_SECRET,
            queues=[QUEUE],
            tasks=[answer],
            name="e2e-generate-worker-again",
            poll_interval=0.1,
            telemetry=AiwatcherClient(service="e2e-generate", base_url=BASE, token=WORKER_SECRET),
        )
        serving = threading.Thread(target=worker.run, name="e2e-generate-worker", daemon=True)
        serving.start()
        held_runs = call("GET", "/api/v1/runs?limit=500")[1] or {}
        peeks_held = [
            run
            for run in held_runs.get("runs", [])
            if str(run.get("caller_run_id") or "").startswith("peek-")
        ]
        restarted_run = followed(
            ok(
                *call(
                    "POST",
                    f"/api/v1/evaluation-runs/{restarting['declaration']['id']}/start",
                )[:2],
                "starting the run asked about before the restart",
            )["execution"]["execution_id"]
        )
        restarted_traces = (
            call(
                "GET",
                f"/api/v1/evaluation-results/{restarting['declaration']['run']['evaluation_id']}",
            )[1]
            or {}
        ).get("traces") or {}
        check(
            45,
            "cases asked before a restart, whose calls the read model no longer holds, are asked "
            "elsewhere all the same: the index of what witnesses saw asked kept them",
            restarted_run["execution"]["state"]["state_type"] == "completed"
            and not peeks_held
            and restarted_traces.get("witnessed_exchange") == 0
            and restarted_traces.get("asked_elsewhere") == len(CAPITALS),
            {"traces": restarted_traces, "peeks_in_read_model": len(peeks_held)},
        )

        def lost_runs() -> int:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}?window_seconds=3600")[:2],
                "reading how many runs were lost",
            )
            row = next(
                (row for row in watched["observed"] if row["variant_id"] == variant_id),
                {},
            )
            return int(row.get("lost_runs") or 0)

        def settled_on(expected: int) -> int:
            deadline = time.monotonic() + 30
            counted = lost_runs()
            while counted != expected and time.monotonic() < deadline:
                time.sleep(1)
                counted = lost_runs()
            return counted

        # A client whose only run the transport loses, closed, and one not closed.
        lonely = AiwatcherClient(
            service="e2e-capitals",
            transport=Dropping(HttpTransport(BASE, token=APPLICATION_SECRET), "lonely-lost"),
            variant_id=variant_id,
        )
        with lonely.run("lonely-lost") as traced, traced.agent("capitals"):
            pass
        lonely.close()
        after_close = settled_on(3)
        killed = AiwatcherClient(
            service="e2e-capitals",
            transport=Dropping(HttpTransport(BASE, token=APPLICATION_SECRET), "killed-lost"),
            variant_id=variant_id,
        )
        with killed.run("killed-lost") as traced, traced.agent("capitals"):
            pass
        killed.flush()
        time.sleep(3)
        unclosed = lost_runs()
        check(
            46,
            "a client whose transport lost its only run says so when it closes, and the window "
            "counts the run lost; one that never closes says nothing before its clock is due",
            after_close == 3 and unclosed == 3,
            {"after_close": after_close, "unclosed": unclosed},
        )

        # A client whose transport was down all along, and the next on its spool.
        spool = home / "spool"
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            nowhere = probe.getsockname()[1]
        down = AiwatcherClient(
            service="e2e-capitals",
            transport=HttpTransport(
                f"http://127.0.0.1:{nowhere}",
                token=APPLICATION_SECRET,
                timeout=0.5,
                spool_dir=spool,
            ),
            variant_id=variant_id,
        )
        with down.run("spooled-lost") as traced, traced.agent("capitals"):
            pass
        down.close()
        kept = sorted(path.name for path in spool.glob("counted-*.json"))
        HttpTransport(BASE, token=APPLICATION_SECRET, spool_dir=spool).close()
        spooled = settled_on(4)
        check(
            47,
            "a count a transport could not deliver is kept on its spool, and the next transport "
            "started on it sends it: the run lost with it is counted",
            len(kept) == 1 and spooled == 4 and not list(spool.glob("counted-*.json")),
            {"kept": kept, "lost_runs": spooled},
        )

        # A client killed without closing, its transport down, on a spool of its own.
        killed_spool = home / "killed-spool"
        opened = home / "killed-opened"
        dying = subprocess.Popen(  # noqa: S603 — this interpreter, this e2e's own client
            [
                sys.executable,
                "-c",
                "import pathlib, sys, time\n"
                "from aiwatcher_sdk import AiwatcherClient, HttpTransport\n"
                "url, spool, opened, variant = sys.argv[1:]\n"
                "client = AiwatcherClient(service='e2e-capitals', variant_id=variant,\n"
                "    transport=HttpTransport(\n"
                "        url, timeout=0.5, flush_interval=60, spool_dir=spool))\n"
                "with client.run('killed-spooled-lost'):\n"
                "    pass\n"
                "pathlib.Path(opened).write_text('opened')\n"
                "time.sleep(120)\n",
                f"http://127.0.0.1:{nowhere}",
                str(killed_spool),
                str(opened),
                variant_id,
            ],
            env={
                **os.environ,
                "PYTHONPATH": str(Path(__file__).resolve().parent.parent / "sdk" / "python"),
            },
        )
        try:
            deadline = time.monotonic() + 30
            while not opened.exists() and time.monotonic() < deadline:
                time.sleep(0.1)
        finally:
            dying.kill()
            dying.wait(timeout=10)
        left = [json.loads(path.read_text()) for path in killed_spool.glob("counted-*.json")]
        HttpTransport(BASE, token=APPLICATION_SECRET, spool_dir=killed_spool).close()
        after_kill = settled_on(5)
        check(
            50,
            "a client killed without closing, whose transport could not deliver its run, leaves "
            "on its spool the count it held as that run started, and the next transport there "
            "sends it: the run is counted lost",
            [count["data"]["runs"] for count in left] == [1]
            and after_kill == 5
            and not list(killed_spool.glob("counted-*.json")),
            {"left": [count["data"] for count in left], "lost_runs": after_kill},
        )

        # The client that never closed, told its clock is due.
        aiwatcher_sdk.COUNTS_EVERY = 1.0
        by_clock = settled_on(6)
        check(
            51,
            "a client that never closes says its count by its own clock once it is due, with no "
            "event after it: the run it lost is counted",
            by_clock == 6,
            {"lost_runs": by_clock},
        )
    finally:
        for relay in gateways:
            relay.shutdown()
        provider.shutdown()
        worker.stop()
        serving.join(timeout=5)
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        if failures:
            print(f"\nkept {home} for the server log")
        else:
            shutil.rmtree(home, ignore_errors=True)

    if failures:
        print(f"\n✗ {len(failures)} claim(s) did not hold")
        return 1
    print("\n✓ a baseline and a candidate generated, scored and compared")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
