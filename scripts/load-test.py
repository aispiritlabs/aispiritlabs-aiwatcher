"""Push many realistic runs at aiwatcher, in batches, and report RSS as it goes."""
import json, os, subprocess, sys, time, urllib.request, uuid

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080")
RUNS = int(sys.argv[1]) if len(sys.argv) > 1 else 2000
CHUNKS = int(sys.argv[2]) if len(sys.argv) > 2 else 24  # llm.chunk events per call

def rss_mb(pid):
    if pid is None:
        return float("nan")
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True)
    return int(out.stdout.strip() or 0) / 1024

def find_pid():
    """The server, whichever profile it was built with."""
    for pattern in ("target/release/aiwatcher$", "target/debug/aiwatcher$", "bin/aiwatcher$"):
        found = subprocess.run(["pgrep", "-f", pattern], capture_output=True, text=True).stdout.split()
        if found:
            return found[0]
    return None

pid = find_pid()

def now():
    import datetime
    return datetime.datetime.now(datetime.UTC).isoformat().replace("+00:00", "Z")

def post(events):
    req = urllib.request.Request(
        f"{BASE}/api/v1/events", data=json.dumps({"events": events}).encode(),
        headers={"content-type": "application/json"}, method="POST")
    urllib.request.urlopen(req, timeout=30).read()

started = time.time()
batch = []
for r in range(RUNS):
    run_id = f"load-{r}"
    conv = f"conv-load-{r % 50}"
    seq = 0
    def ev(t, data, agent=None):
        global seq
        seq += 1
        e = {"event_id": f"{run_id}-{seq}", "event_type": t, "occurred_at": now(),
             "run_id": run_id, "conversation_id": conv, "sequence": seq,
             "source": {"service": "load", "sdk": "python"}, "data": data}
        if agent: e["agent_id"] = agent
        batch.append(e)
    ev("run.started", {})
    for a in ("planner", "researcher"):
        ev("agent.started", {}, a)
        ev("llm.started", {"call_id": f"c{a}", "provider": "anthropic", "model": "claude-opus-5"}, a)
        for i in range(CHUNKS):
            ev("llm.chunk", {"call_id": f"c{a}", "text": "token " * 4}, a)
        ev("llm.completed", {"call_id": f"c{a}", "provider": "anthropic", "model": "claude-opus-5",
                             "prompt_tokens": 1200, "completion_tokens": 300, "cached_tokens": 400}, a)
        ev("tool.started", {"call_id": f"t{a}", "tool_name": "web_search"}, a)
        ev("tool.completed", {"call_id": f"t{a}", "tool_name": "web_search"}, a)
        ev("agent.completed", {}, a)
    ev("run.completed", {"status": "succeeded"})

    if len(batch) >= 600:
        post(batch); batch = []
    if r % 500 == 0 and r:
        print(f"  {r} runs  RSS {rss_mb(pid):.0f} MB", flush=True)
if batch: post(batch)

time.sleep(4)
print(f"done: {RUNS} runs in {time.time()-started:.0f}s  RSS {rss_mb(pid):.0f} MB")
