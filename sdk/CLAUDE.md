# The SDKs

Four distributions publish to aiwatcher, and they do not depend on one another:
`python` (the telemetry client plus the registry clients), `agentic` (the
workflow engine and agent core agents are built from), `typescript`, and `go`.
`just sdk-check`, `just agentic-check` and `just sdk-go-check` are their gates;
the TypeScript one is checked by the panel's build and its own tests.

The Go SDK (`sdk/go`, module `…/sdk/go/aiwatcher`, Go 1.25, standard library
only) publishes runs, the agents in them and their model and tool calls, and
carries two registry clients beside the telemetry one: `Archive` records what
was said into the encrypted conversation archive, and `Prompts` publishes the
prompt a service runs on so a trace can name the version that answered. It
follows the other SDKs on field names, id rules and event types, and `tests`
validate every envelope it produces against `contracts/envelope.schema.json`.
What it does not publish yet: declared workflows, evaluations, agent messages,
and a client's count of the runs it opened per variant. Publishing never blocks,
the queue is bounded and drops are counted, telemetry is not retried, and the
registry clients return every error — the split below, in a third language.

`sdk/python` is a `uv` project with its own `pyproject.toml`, and its
dependencies are **split by half**. The telemetry client — `aiwatcher_sdk`
itself, which is what an instrumented agent imports — depends on nothing and
stays on `urllib`: it is imported into processes that already pinned `httpx` and
`pydantic`, and it must never take an agent down. The four **registry** clients
depend on `httpx` and `tenacity`, because they run in training jobs and deploy
steps rather than in a request path, every method raises, and the retry policy
is the thing most worth writing once — `aiwatcher_sdk/api.py` is that one place.
`uv.lock` is committed. `just sdk-check` runs `ruff format --check`, `ruff
check`, `mypy --strict` and `pytest`; CI runs the same on Python 3.13, which is
the floor `requires-python` claims. The lint set is the one `planner` selects,
deliberately: the two repositories are worked on together, and a lint that fires
in one and not the other is a lint people learn to ignore.

`sdk/agentic` is the **third** distribution, `aiwatcher-agentic`: what agents
are built from, moved from `ai_spirit_agent` (AW-2) — the workflow engine
(`aiwatcher_agentic.workflow`: messages, deciders, event stores, sagas), the
agent core beside it (`Agent`, tools, messages, prompt builders), and the
runtime that composes and hosts them (`aiwatcher_agentic.runtime`:
`AgenticRuntime`, its stores, `hosted`, and the transport on which a hop between
two agents is a run of the target's workflow). Nothing in `aiwatcher-sdk`
imports it, so importing telemetry never imports an agent; the runtime reaches
aiwatcher through `aiwatcher-sdk`, imported only when used — the `[aiwatcher]`
extra. The engine is the standard library alone and the core adds `structlog`
and `orjson`; what none of them may bring is a model stack, so where they need
one they declare a port — `model.ModelSource` for a model, `tracer.LLMTracer`
for a tracer, `prompts.PromptSource` for a prompt read by name,
`runtime.config.RuntimeSettings` for settings, `AgentRun` for what the engine
reads off a turn — and the adapters (`providers`, the MLflow tracer and
registry, the `.env` settings) stay with the application. `test_agent_port` and
`test_runtime_port` fail if importing either pulls a stack in. A store handed no
path goes under the working directory, never beside the code — derived from its
own file, it named this checkout. Its records keep the wire names they had
before the move — `serialization.WIRE_PREFIX` — because stored rows carry them,
and `tests/fixtures/` holds them to those bytes. `just agentic-check` runs the
same four checks on its own lock.

The telemetry client and the registry client have **opposite** failure policies,
and that is the design: telemetry must never take an agent down, so
`HttpTransport` swallows and counts; reading the prompt a service is about to
run on is the work, so every `PromptRegistry` method raises.
`aiwatcher_sdk/integrations/deepeval.py` never imports deepeval — it reads the
report structurally, so a DeepEval release is not an SDK release. The same rule
holds for torch, with one deliberate exception:
`ExportDataset.as_torch_dataloader` imports it *inside the method*, because
handing back a `DataLoader` is the one thing that cannot be done structurally.

An annotation export is read the shape PyTorch reads a dataset.
`data_registry.build_dataloader(project)` freezes the project and hands back the
loader — the `Export` with the registry attached, whose `source` is the
`project@sha256` a run records and whose `excluded_samples` say what was left
out and why. `dataloader.get_split("test")` is a `SplitView` — a
`Sequence[Sample]` that also answers `get_groups()` and `get_counts()` — and
`.as_dataset(...)` turns it into the map-style dataset a `DataLoader` takes,
through the registry the loader remembers being read from rather than one the
caller repeats. Everything on that path pickles, because a `DataLoader` with
workers puts it across a process boundary under `spawn`. The split is where the
group rule already lives, so nothing downstream gets a second chance to decide
which subjects a model may see.

**Registry clients are named by one rule, and a new method follows it.** A
method is a verb phrase: a read is `get_<noun>`, `iter_<noun>` when it pages, a
conversion is `as_<noun>`, and `build_<noun>` asks the server to make something;
a field or a property is a noun; a collection is named for what it holds
(`samples`, `excluded_samples`). Writes keep the verb that says what they do. A
bare-noun method — `split()`, `families()`, `counts()`, `job()`, `policy()` —
reads like a field and has to be looked up to find out that it is a request,
which is why none is left. The telemetry client's scopes (`client.run`,
`run.agent`, `agent.llm`) are a different idiom, a `with` block that names what
it opens, and are deliberately not covered.

Two more rules hold everywhere in `sdk/python`. **A class name never starts with
an underscore** — internal means absent from `__all__`, and a leading underscore
is the same statement made more weakly, which then leaks into every annotation
that mentions it (`list[_Context]` in a neighbouring module is a private name
crossing a module boundary in public). And **a `@contextmanager` is annotated
`Generator[T, None, None]`, never `Iterator[T]`**: the decorated function is a
generator, `contextlib` throws exceptions back into it at the `yield`, and the
three-argument spelling is written out: `Generator[T]` needed PEP 696 defaults
while the floor was 3.11, and the full form stayed the house spelling when it
rose to 3.13, which is why ruff's `UP043` is ignored.

`aiwatcher_sdk/annotations` is a **package sliced by noun**, and the slicing
rule is the Rust one above: a change to what one thing *is* touches one file.
`errors` → `split` (the rule that deals a group a side) → `sample` (`Sample` and
`ExcludedSample`, the two halves of a manifest) → `image_source` → `view`
(`SplitView`) → `export` (`Export`) → `registry` (`AnnotationRegistry`, the only
file that knows a network exists), with `__init__` as the door that re-exports
all of it. Every import points up that list. The one thing holding it straight
is `image_source::ImageSource` — the three reads a dataset needs, a project's
schema, one revision's shapes and an image's bytes. A manifest carries the
source it was read from and the registry hands back manifests, so written
against the concrete client those two would import each other; written against
the protocol they meet at the abstraction. It is also what makes a cache, an
offline corpus or a test double substitutable for the client without inheriting
from it, which
`test_a_dataset_reads_through_the_port_rather_than_through_the_client` is there
to keep true.

`aiwatcher_sdk/serving` is the one thing here that reads a decision back out and
acts on it rather than recording one: it resolves the `production` label and
serves what it names. Its shape is a split — `serving/server.py` holds what
every framework needs (resolve, verify, warm, bound, validate, watch, roll back,
report) and `serving/runtimes/` holds what one framework needs, which is four
members and a loader. That is why a runtime is cheap to add and why the rollout
exists once. `weights` needs nothing; `onnx` needs the `[onnx]` extra and is
imported lazily, so a host that registers no ONNX loader never sees the wheel.
Its gates are tested against a stub session rather than the real one — they are
pure functions of what a session says about itself, and `just onnx-version` is
what runs a real graph.

`aiwatcher_sdk/gateway` (`aiwatcher-gateway`) is the witness for what an
application's own telemetry cannot vouch for: an OpenAI-compatible relay in
front of a provider, holding the provider's key so the application need not,
that publishes under its own token a run naming the caller's
(`Aiwatcher-Caller-Run`) with the model the provider said served the call and
whether the request's text holds the template of the prompt version the caller
names (`Aiwatcher-Prompt`) — found exactly, rendered with the values the caller
sends in the body field it removes before the provider sees the request
(`LlmCall.caller_body`), or by the template's literal parts — and whether the
request held nothing else. The same field may say how the caller takes its
answer out of the reply — steps from a closed vocabulary (a JSON pointer, text
between markers, a line, a fenced block, stripped, lower-cased, a number, a
label's word), in turn or as alternatives, and never a pattern a caller wrote —
which the gateway takes the same way, with the function the caller takes it with
(`gateway.extracted`), and which of its values it took out of another in those
steps (`derived`), which the gateway takes out again. It relays the deployment's
tools the same way, at `/tools/<name>` to the URL the deployment named and never
one a caller names, digesting each part of the arguments and what came back —
and a tool the application calls directly is witnessed on its own host by
`ToolWitness`, in Python or TypeScript, digesting under the gateway's witness
key — held by a host publishing under a token of its own where the deployment
says so (`AIWATCHER_WITNESS_DIGESTS`) — while a tool the application would
compute itself can be a function the gateway answers, published with the sha256
of its source file (`gateway.tool_code`), which a variant may pin — as it may
the digest a URL's service names in `Aiwatcher-Tool-Code`, or a host hands
`ToolWitness` as `code`. Named in `caller_body(ordered=…)`, a judge's candidates
are moved into the order of their digests under its key — which the caller
cannot compute — before the request is relayed, and the reply says where each
went (`Aiwatcher-Placed`). It is the telemetry client's half — the standard
library and nothing else — and it publishes neither the request nor the reply:
only keyed digests of each message, of the values it found rendered — again as a
reply's are made, so a value that is what a model already replied reads as that
reply — and of each reply, under a key derived from its own credential, which
the deployment can test an answer and a case's input against and a reader of the
log cannot test a guess against (`aiwatcher_core::witness`, byte for byte) — and
what it asked digested a second time normalised (`gateway.normalized`: NFKC,
lower case, no punctuation, one space, the same in Rust and TypeScript), so a
question asked again in other case or spacing is found. It posts the witness
before the reply ends, because the caller reads to the connection's close, so a
call cannot end on the log before its witness is there.
