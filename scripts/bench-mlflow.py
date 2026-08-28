"""Same agent workload, three tracer configurations, measured.

Drives the agent's real `LLMTracer` surface — the same calls `agentic` makes —
so the comparison is "what does this backend cost for the work my agent
actually does", not a synthetic microbenchmark.

  mlflow    MlflowLLMTracer against a local sqlite store
  aiwatcher AiwatcherTracer against a running aiwatcher
  tee       both, which is what create_tracer() wires when AIWATCHER_URL is set
"""
import os, resource, subprocess, sys, time
from dataclasses import dataclass

MODE = sys.argv[1]
RUNS = int(sys.argv[2])

@dataclass(frozen=True, slots=True)
class FakeResponse:
    text: str = "ok"
    model: str = "claude-opus-5"
    request_id: str = ""
    finish_reason: str = "stop"
    prompt_tokens: int = 1200
    completion_tokens: int = 300
    total_tokens: int = 1500
    latency_ms: float = 900.0

def build():
    if MODE == "mlflow":
        from agentic.observability import build_tracer
        return build_tracer(enabled=True, backend="mlflow",
                            tracking_uri=os.environ["MLFLOW_TRACKING_URI"])
    if MODE == "aiwatcher":
        from aiwatcher_sdk import HttpTransport
        from aiwatcher_sdk.integrations.agentic import aiwatcher_tracer
        transport = HttpTransport(
            os.environ["AIWATCHER_URL"],
            batch_size=int(os.environ.get("BENCH_BATCH", "500")),
            flush_interval=0.2,
            queue_size=int(os.environ.get("BENCH_QUEUE", "10000")),
        )
        globals()["TRANSPORT"] = transport
        return aiwatcher_tracer(service="bench", transport=transport)
    from agentic.observability import build_tracer
    from aiwatcher_sdk.integrations.agentic import aiwatcher_tracer, tee
    return tee(build_tracer(enabled=True, backend="mlflow",
                            tracking_uri=os.environ["MLFLOW_TRACKING_URI"]),
               aiwatcher_tracer(service="bench"))

baseline = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / (1024 * 1024)
tracer = build()
after_import = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / (1024 * 1024)

def invoke(**kwargs):
    return FakeResponse()

started = time.time()
for r in range(RUNS):
    with tracer.workflow(name="bench", session_id=f"conv-{r % 50}"):
        for agent in ("planner", "researcher"):
            with tracer.agent(name=agent, agent_id=agent):
                tracer.llm(name="chat", model="claude-opus-5",
                           messages=[{"role": "user", "content": "hi"}],
                           invoke=invoke, provider="anthropic")
                with tracer.step(name="tool.web_search", span_type="TOOL",
                                 attributes={"tool_name": "web_search"}):
                    pass
    if r and r % 200 == 0:
        rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / (1024 * 1024)
        rate = r / (time.time() - started)
        print(f"  {r} runs  {rate:.0f} runs/s  python peak RSS {rss:.0f} MB", flush=True)

elapsed = time.time() - started
flush_started = time.time()
tracer.flush()
flush = time.time() - flush_started

# Enqueueing is not the same as being able to see the data. For aiwatcher, wait
# until the server's read model actually reflects the runs; that is the number
# worth comparing against MLflow's synchronous write.
queryable = None
if MODE in ("aiwatcher", "tee"):
    import json, urllib.request
    deadline = time.time() + 120
    while time.time() < deadline:
        with urllib.request.urlopen("http://127.0.0.1:8080/api/v1/metrics") as response:
            retained = json.load(response)["window"]["runs_retained"]
        if retained >= min(RUNS, 5000):
            break
        time.sleep(0.2)
    queryable = time.time() - started

peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / (1024 * 1024)
dropped = getattr(globals().get("TRANSPORT"), "dropped", 0)
print(f"RESULT dropped={dropped} mode={MODE} runs={RUNS} agent_blocked={elapsed:.1f}s flush={flush:.1f}s "
      f"queryable={queryable if queryable is None else round(queryable,1)} "
      f"rate={RUNS/elapsed:.0f}runs/s baseline_rss={baseline:.0f}MB "
      f"after_import_rss={after_import:.0f}MB python_peak_rss={peak:.0f}MB")
