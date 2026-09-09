#!/usr/bin/env python3
"""Real Rust + PostgreSQL + Python recovery gate. Uses an isolated disposable database.

Run after `cargo build --bin aiwatcher --features postgres` and `just postgres-up`:
  cd sdk/python && uv run python ../../scripts/check-worker-runtime.py
The real lease lasts five minutes; this test does not change it or rewrite claim rows.
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk/python/examples"))
from worker_workflow import build_runtime


def stop(process: subprocess.Popen, *, kill: bool = False) -> None:
    if process.poll() is None:
        process.kill() if kill else process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def ready(url: str, process: subprocess.Popen) -> None:
    for _ in range(200):
        if process.poll() is not None:
            raise RuntimeError("server exited; inspect its log")
        try:
            if httpx.get(url + "/healthz").is_success:
                return
        except httpx.HTTPError:
            pass
        time.sleep(0.1)
    raise TimeoutError("server did not become ready")


def check_observability(api: httpx.Client, execution_id: str, attempts: int) -> dict[str, int]:
    for _ in range(100):
        graph = api.get(f"/api/v1/workflow-executions/{execution_id}").json()
        spans = api.get("/api/v1/spans", params={"run_id": execution_id, "limit": 100}).json()
        agents = [
            span for span in spans.get("spans", []) if span.get("operation") == "invoke_agent"
        ]
        steps = {
            span["span_id"]: span
            for span in spans.get("spans", [])
            if span.get("operation") == "step"
        }
        if (
            len(agents) == 2
            and len(graph.get("nodes", [])) == 2
            and all(node["status"] == "succeeded" for node in graph["nodes"])
            and len(steps) == attempts
        ):
            break
        time.sleep(0.1)
    assert len(graph["nodes"]) == 2, graph
    assert all(node["status"] == "succeeded" for node in graph["nodes"]), graph
    steps = {span["span_id"]: span for span in spans["spans"] if span.get("operation") == "step"}
    assert len(steps) == attempts, spans
    assert len(agents) == 2, spans
    assert len({span["trace_id"] for span in spans["spans"]}) == 1, spans
    for agent in agents:
        parent = steps[agent["parent_span_id"]]
        assert parent["name"] == agent["agent_id"], spans
    return {"graph_nodes": 2, "step_spans": len(steps), "agent_spans": len(agents)}


def protocol_case(url, env, restart, worker_log) -> None:
    from aiwatcher_sdk.worker import Worker
    from worker_workflow import acquire

    class LostReply(httpx.BaseTransport):
        def __init__(self):
            self.inner = httpx.HTTPTransport()
            self.reports = 0

        def handle_request(self, request):
            response = self.inner.handle_request(request)
            if request.url.path.endswith("/result"):
                self.reports += 1
                if self.reports == 1:
                    response.read()
                    assert response.status_code == 200, response.text
                    response.close()
                    restart()
                    print(
                        "Committed result, restarted Rust, then dropped its HTTP reply.", flush=True
                    )
                    raise httpx.ReadTimeout("injected loss after durable commit")
            return response

        def close(self):
            self.inner.close()

    with build_runtime() as runtime:
        handle = runtime.run("demo.worker-import@1", parameters={"number": 7})
        transport = LostReply()
        with (
            httpx.Client(transport=transport) as client,
            Worker(url, queues=["demo"], tasks=[acquire], client=client) as worker,
        ):
            assert worker.run_attempt(f"{handle.execution_id}/acquire/1")
        assert transport.reports == 2, "the committed report must be acknowledged on redelivery"
        for attempt, expected_exit in [(1, 1), (2, 0)]:
            completed = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "aiwatcher_sdk.runtime",
                    "--factory",
                    "worker_workflow:build_runtime",
                    "--pool",
                    "local",
                    "--attempt",
                    f"{handle.execution_id}/persist/{attempt}",
                ],
                env=env,
                stdout=worker_log,
                stderr=subprocess.STDOUT,
                timeout=45,
            )
            assert completed.returncode == expected_exit, completed.returncode
        view = handle.wait(timeout=20)
        assert view["execution"]["state"]["state_type"] == "completed", view
        history = handle.history(limit=100)["messages"]
        completions = [
            row
            for row in history
            if row["direction"] == "output"
            and row["message"].get("event") == "step_completed"
            and row["message"].get("step_id") == "acquire"
        ]
        assert len(completions) == 1, completions
        with httpx.Client(base_url=url) as api:
            observation = check_observability(api, handle.execution_id, 3)
        print(
            json.dumps(
                {
                    "mode": "protocol",
                    "report_deliveries": transport.reports,
                    "acquire_completions": len(completions),
                    "cli_exit_codes": [1, 0],
                    **observation,
                }
            ),
            flush=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--protocol",
        action="store_true",
        help="lost reply, Rust restart and Runtime per-attempt processes",
    )
    args = parser.parse_args()
    database = f"aiwatcher_sdk_e2e_{os.getpid()}"
    postgres = ["docker", "exec", "aiwatcher-postgres"]
    subprocess.run([*postgres, "createdb", "-U", "aiwatcher", database], check=True)
    processes: list[subprocess.Popen] = []
    try:
        with tempfile.TemporaryDirectory(prefix="aiwatcher-sdk-e2e-") as directory:
            data = Path(directory)
            with socket.socket() as address:
                address.bind(("127.0.0.1", 0))
                port = address.getsockname()[1]
            url = f"http://127.0.0.1:{port}"
            os.environ["AIWATCHER_URL"] = url
            env = {
                **os.environ,
                "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
                "AIWATCHER_DATA_DIR": directory,
                "AIWATCHER_BUS": "wal",
                "AIWATCHER_AUTH_MODE": "none",
                "AIWATCHER_INGEST_ENABLED": "true",
                "AIWATCHER_PROMPT_STORE": "file",
                "AIWATCHER_WORKFLOW_STORE": "postgres",
                "AIWATCHER_WORKFLOW_POSTGRES_URL": f"postgres://aiwatcher:aiwatcher@127.0.0.1:5433/{database}",
                "PYTHONPATH": str(ROOT / "sdk/python/examples"),
            }
            env.pop("AIWATCHER_TOKEN", None)
            with (
                (data / "server.log").open("w+") as server_log,
                (data / "worker.log").open("w+") as worker_log,
            ):

                def server() -> subprocess.Popen:
                    process = subprocess.Popen(
                        [str(ROOT / "target/debug/aiwatcher")],
                        env=env,
                        stdout=server_log,
                        stderr=subprocess.STDOUT,
                    )
                    processes.append(process)
                    ready(url, process)
                    return process

                def worker(block: bool = False) -> subprocess.Popen:
                    worker_env = dict(env)
                    if block:
                        worker_env["AIWATCHER_DEMO_BLOCK_FILE"] = str(data / "claimed")
                    process = subprocess.Popen(
                        [
                            sys.executable,
                            "-m",
                            "aiwatcher_sdk.runtime",
                            "--factory",
                            "worker_workflow:build_runtime",
                        ],
                        env=worker_env,
                        stdout=worker_log,
                        stderr=subprocess.STDOUT,
                    )
                    processes.append(process)
                    return process

                try:
                    service = server()
                    if args.protocol:

                        def restart():
                            nonlocal service
                            stop(service)
                            service = server()

                        protocol_case(url, env, restart, worker_log)
                        return
                    with build_runtime() as runtime:
                        handle = runtime.run(
                            "demo.worker-import@1",
                            parameters={"number": 7},
                            idempotency_key="recovery-gate",
                        )
                        original = worker(block=True)
                        for _ in range(150):
                            if (data / "claimed").exists():
                                break
                            if original.poll() is not None:
                                raise RuntimeError("first worker exited before claiming")
                            time.sleep(0.1)
                        assert (data / "claimed").exists(), "worker never entered its task"
                        stop(original, kill=True)
                        print(
                            "Killed worker during acquire; restarting Rust against the same history.",
                            flush=True,
                        )
                        stop(service)
                        server()
                        worker()
                        print(
                            "Replacement is waiting for the real 300-second lease to expire.",
                            flush=True,
                        )
                        view = handle.wait(timeout=380, poll_interval=1)
                        assert view["execution"]["state"]["state_type"] == "completed", view
                        messages = []
                        after = 0
                        while True:
                            page = handle.history(after=after, limit=100)
                            messages.extend(page["messages"])
                            if page["next_after"] is None:
                                break
                            after = page["next_after"]
                        assert any(m["message"].get("event") == "step_failed" for m in messages)
                        assert any(
                            m["message"].get("event") == "execution_completed" for m in messages
                        )
                        with httpx.Client(base_url=url) as api:
                            attempts = sum(
                                len(step["attempts"]) for step in view["execution"]["steps"]
                            )
                            observation = check_observability(api, handle.execution_id, attempts)
                        print(
                            json.dumps(
                                {
                                    "execution_id": handle.execution_id,
                                    "state": "completed",
                                    "history_messages": len(messages),
                                    **observation,
                                }
                            ),
                            flush=True,
                        )
                except BaseException:
                    for label, log in (("server", server_log), ("worker", worker_log)):
                        log.flush()
                        log.seek(0)
                        print(f"{label} log:\n{log.read()[-12000:]}", file=sys.stderr)
                    raise
                finally:
                    for process in reversed(processes):
                        stop(process)
    finally:
        subprocess.run([*postgres, "dropdb", "-U", "aiwatcher", "--force", database], check=True)


if __name__ == "__main__":
    main()
