/**
 * aiwatcher client for TypeScript agents.
 *
 * The contract is the envelope in `contracts/envelope.schema.json`, not this
 * library. Anything that can produce that JSON and get it onto the Laser topic
 * is a valid producer; this exists so the common case is three lines.
 *
 * Mirrors the Python SDK deliberately — the same scope shapes, the same field
 * names, the same rules about ids — because a team running agents in both
 * languages should not have to hold two mental models.
 */

export const SCHEMA_VERSION = 1;

export type Sdk = 'python' | 'typescript' | 'rust' | (string & {});

export interface Source {
  service: string;
  instance?: string;
  sdk: Sdk;
}

/** The wire form. Everything optional is filled in by the backend. */
export interface EventEnvelope {
  schema_version: number;
  kind: 'Event';
  event_id: string;
  event_type: string;
  occurred_at: string;
  run_id: string;
  conversation_id?: string;
  workflow_id?: string;
  workflow_run_id?: string;
  agent_id?: string;
  sequence?: number;
  trace_id?: string;
  span_id?: string;
  parent_span_id?: string;
  correlation_id?: string;
  causation_id?: string;
  source: Source;
  data: Record<string, unknown>;
}

export interface Transport {
  send(events: EventEnvelope[]): void;
  close(): Promise<void>;
}

/** Drops everything. The default, so importing this never breaks a test. */
export class NullTransport implements Transport {
  send(): void {}
  async close(): Promise<void> {}
}

export interface HttpTransportOptions {
  baseUrl: string;
  /**
   * Sent as `Authorization: Bearer`.
   *
   * Needed only against an instance with single sign-on on. A producer runs
   * where nobody can complete an interactive login and reaches the server
   * directly, so it carries a token of its own — which grants the editor role
   * and never admin. Defaults to `AIWATCHER_TOKEN` where there is an
   * environment to read it from.
   */
  token?: string;
  batchSize?: number;
  flushIntervalMs?: number;
  /**
   * Bounded on purpose: telemetry must not be able to exhaust the agent's
   * memory. A full queue drops events and reports it, which beats an OOM.
   */
  queueSize?: number;
}

/**
 * Posts batches to `POST /api/v1/events`.
 *
 * The fallback path, for producers that cannot reach Laser directly. Batched
 * and non-blocking: an agent should never wait on telemetry, and flushing per
 * token would cost a round trip per token.
 */
export class HttpTransport implements Transport {
  readonly #url: string;
  readonly #token: string | undefined;
  readonly #batchSize: number;
  readonly #flushIntervalMs: number;
  readonly #queueSize: number;
  #pending: EventEnvelope[] = [];
  #timer: ReturnType<typeof setTimeout> | undefined;
  #dropped = 0;

  constructor(options: HttpTransportOptions) {
    this.#url = `${options.baseUrl.replace(/\/$/, '')}/api/v1/events`;
    // `globalThis.process` rather than `process`: this runs in a browser as
    // well, and reaching for a bare `process` there is a ReferenceError rather
    // than an undefined.
    this.#token =
      options.token ??
      (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env
        ?.AIWATCHER_TOKEN;
    this.#batchSize = options.batchSize ?? 64;
    this.#flushIntervalMs = options.flushIntervalMs ?? 1000;
    this.#queueSize = options.queueSize ?? 10_000;
  }

  /** Events discarded because the queue was full. */
  get dropped(): number {
    return this.#dropped;
  }

  send(events: EventEnvelope[]): void {
    for (const event of events) {
      if (this.#pending.length >= this.#queueSize) {
        this.#dropped += 1;
        continue;
      }
      this.#pending.push(event);
    }
    if (this.#pending.length >= this.#batchSize) {
      void this.#flush();
      return;
    }
    this.#timer ??= setTimeout(() => void this.#flush(), this.#flushIntervalMs);
  }

  async close(): Promise<void> {
    await this.#flush();
  }

  async #flush(): Promise<void> {
    if (this.#timer !== undefined) {
      clearTimeout(this.#timer);
      this.#timer = undefined;
    }
    const batch = this.#pending;
    if (batch.length === 0) return;
    this.#pending = [];

    try {
      await fetch(this.#url, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          ...(this.#token ? { authorization: `Bearer ${this.#token}` } : {}),
        },
        body: JSON.stringify({ events: batch }),
        keepalive: true,
      });
    } catch (error) {
      // Telemetry must never take the agent down with it. A 401 is not thrown
      // by `fetch` and lands nowhere: it means the instance has single sign-on
      // on and this producer has no token, which is a variable to set rather
      // than a network problem to wait out.
      this.#dropped += batch.length;
      console.warn(`[aiwatcher] dropped ${batch.length} events`, error);
    }
  }
}

interface Context {
  runId: string;
  conversationId?: string;
  workflowId?: string;
  /**
   * One execution of that workflow. Separate from `workflowId` because an
   * execution can outlive a run: a stage-per-pod orchestrator gives every
   * stage its own process, and this is what joins them back into one graph.
   */
  workflowRunId?: string;
  agentId?: string;
  correlationId: string;
  causationId?: string;
}

const newId = () => globalThis.crypto.randomUUID();
const now = () => new Date().toISOString();

export interface ClientOptions {
  service: string;
  instance?: string;
  transport?: Transport;
  baseUrl?: string;
  /** See {@link HttpTransportOptions.token}. */
  token?: string;
}

export class AiwatcherClient {
  readonly #transport: Transport;
  readonly #source: Source;

  constructor(options: ClientOptions) {
    this.#transport =
      options.transport ??
      (options.baseUrl
        ? new HttpTransport({
            baseUrl: options.baseUrl,
            ...(options.token ? { token: options.token } : {}),
          })
        : new NullTransport());
    this.#source = {
      service: options.service,
      sdk: 'typescript',
      ...(options.instance ? { instance: options.instance } : {}),
    };
  }

  /**
   * Publish one event. Returns its id.
   *
   * `occurredAt` overrides the clock. Only worth passing for something
   * reported after the fact — an evaluation summarised once it has already run
   * — where stamping *now* on the start would report a duration of zero for
   * work that took twenty minutes.
   */
  emit(
    eventType: string,
    context: Context,
    data: Record<string, unknown> = {},
    occurredAt?: string,
  ): string {
    const eventId = newId();
    this.#transport.send([
      {
        schema_version: SCHEMA_VERSION,
        kind: 'Event',
        event_id: eventId,
        event_type: eventType,
        occurred_at: occurredAt ?? now(),
        run_id: context.runId,
        correlation_id: context.correlationId,
        source: this.#source,
        data,
        ...(context.conversationId ? { conversation_id: context.conversationId } : {}),
        ...(context.workflowId ? { workflow_id: context.workflowId } : {}),
        ...(context.workflowRunId ? { workflow_run_id: context.workflowRunId } : {}),
        ...(context.agentId ? { agent_id: context.agentId } : {}),
        ...(context.causationId ? { causation_id: context.causationId } : {}),
      },
    ]);
    return eventId;
  }

  /**
   * One execution of an agent. Becomes one trace.
   *
   * `conversationId` groups runs by who is talking; `workflowId` groups them by
   * what is being executed, so the same orchestration is comparable across
   * sessions.
   *
   * `run.failed` is emitted for any thrown value, including a cancellation —
   * a cancelled run that never reports an end looks identical to a hung one.
   */
  async run<T>(
    runId: string,
    options:
      | {
          conversationId?: string;
          workflowId?: string;
          workflowRunId?: string;
          correlationId?: string;
        }
      | undefined,
    body: (run: RunScope) => Promise<T>,
  ): Promise<T> {
    const context: Context = {
      runId,
      correlationId: options?.correlationId ?? newId(),
      ...(options?.conversationId ? { conversationId: options.conversationId } : {}),
      ...(options?.workflowId ? { workflowId: options.workflowId } : {}),
      ...(options?.workflowId && options?.workflowRunId
        ? { workflowRunId: options.workflowRunId }
        : {}),
    };
    this.emit('run.started', context);
    try {
      const result = await body(new RunScope(this, context));
      this.emit('run.completed', context, { status: 'succeeded' });
      return result;
    } catch (error) {
      this.emit('run.failed', context, { error: String(error), status: 'failed' });
      throw error;
    }
  }

  /**
   * One execution of an orchestration, and the shape it is executing.
   *
   * Declaring the shape is the point. Without it the graph can only ever show
   * the stages that have already run, and "which stage has this not reached"
   * is the question somebody watching a pipeline is asking.
   *
   * The declaration is idempotent — its version is a hash of the topology — so
   * publishing it on every execution costs nothing and is what keeps the
   * catalog alive across retention eviction. Declare unconditionally.
   *
   * `executionId` joins several processes into one traversal. Omit it and the
   * run *is* the execution, which is right whenever the whole workflow runs in
   * one process.
   */
  async workflow<T>(
    workflowId: string,
    options: WorkflowOptions | undefined,
    body: (flow: WorkflowScope) => Promise<T>,
  ): Promise<T> {
    const nodes = normalizeNodes(options?.nodes);
    const edges = normalizeEdges(options?.edges);
    const context: Context = {
      runId: options?.runId ?? newId(),
      correlationId: newId(),
      workflowId,
      ...(options?.conversationId ? { conversationId: options.conversationId } : {}),
      ...(options?.executionId ? { workflowRunId: options.executionId } : {}),
    };
    this.emit('run.started', context);
    if (nodes.length > 0 || edges.length > 0) {
      this.emit('workflow.declared', context, {
        name: options?.name ?? workflowId,
        version: await topologyVersion(nodes, edges),
        nodes,
        edges,
      });
    }
    try {
      const result = await body(new WorkflowScope(this, context));
      this.emit('run.completed', context, { status: 'succeeded' });
      return result;
    } catch (error) {
      this.emit('run.failed', context, { error: String(error), status: 'failed' });
      throw error;
    }
  }

  /**
   * One execution of an evaluation suite. Becomes a report, not a trace.
   *
   * `eval.*` events ride the same log as everything else and are folded apart
   * from it: they produce no span, no trace record and no row in the runs
   * list. What they produce is the evaluation view — parameters, metrics,
   * per-case scores and whatever document you attach.
   *
   * `dataset` is what makes two reports comparable. The backend compares a
   * report only against the previous one **on the same dataset**, so an
   * unversioned suite silently compares against itself.
   */
  async evaluation<T>(
    suite: string,
    options: EvaluationOptions | undefined,
    body: (evaluation: EvaluationScope) => Promise<T>,
  ): Promise<T> {
    const context: Context = { runId: options?.evaluationId ?? newId(), correlationId: newId() };
    const base: Record<string, unknown> = {
      suite,
      ...(options?.dataset ? { dataset: options.dataset } : {}),
      ...(options?.variant ? { variant: options.variant } : {}),
      ...(options?.params ? { params: stringify(options.params) } : {}),
    };

    this.emit('eval.started', context, base);
    const evaluation = new EvaluationScope(this, context, base);
    try {
      const result = await body(evaluation);
      this.emit('eval.completed', context, { ...base, ...evaluation.payload() });
      return result;
    } catch (error) {
      this.emit('eval.failed', context, {
        ...base,
        ...evaluation.payload(),
        error: String(error),
      });
      throw error;
    }
  }

  /**
   * Publish a finished evaluation in one call. Returns its id.
   *
   * The direct replacement for an MLflow `start_run` / `log_params` /
   * `log_metrics` / `log_dict` block: same four pieces, no server, and the
   * result lands next to the traces the evaluated agent produced.
   *
   * `durationMs` back-dates the start. Without it the report is stamped as
   * instantaneous — honest, since nothing said when it began, and useless for
   * anything that looks at how long a suite takes.
   */
  recordEvaluation(options: RecordEvaluationOptions): string {
    const context: Context = { runId: options.evaluationId ?? newId(), correlationId: newId() };
    const base: Record<string, unknown> = {
      suite: options.suite,
      ...(options.dataset ? { dataset: options.dataset } : {}),
      ...(options.variant ? { variant: options.variant } : {}),
      ...(options.params ? { params: stringify(options.params) } : {}),
    };

    const startedAt =
      options.durationMs === undefined
        ? undefined
        : new Date(Date.now() - Math.max(options.durationMs, 0)).toISOString();
    this.emit('eval.started', context, base, startedAt);

    this.emit('eval.completed', context, {
      ...base,
      ...(options.metrics ? { metrics: options.metrics } : {}),
      ...(options.report ? { report: options.report } : {}),
      ...(options.casesTotal === undefined ? {} : { cases_total: options.casesTotal }),
      ...(options.casesPassed === undefined ? {} : { cases_passed: options.casesPassed }),
      ...(options.casesTotal === undefined || options.casesPassed === undefined
        ? {}
        : { cases_failed: Math.max(options.casesTotal - options.casesPassed, 0) }),
    });
    return context.runId;
  }

  async close(): Promise<void> {
    await this.#transport.close();
  }
}

export interface EvaluationOptions {
  evaluationId?: string;
  /** What the suite was measured on. Without it, nothing is comparable. */
  dataset?: string;
  /** What was under test: a prompt version, a model, a checkout. */
  variant?: string;
  params?: Record<string, unknown>;
}

export interface RecordEvaluationOptions extends EvaluationOptions {
  suite: string;
  metrics?: Record<string, number>;
  report?: Record<string, unknown>;
  casesTotal?: number;
  casesPassed?: number;
  durationMs?: number;
}

/**
 * The scope `AiwatcherClient.evaluation` yields.
 *
 * Cases are published as they are scored, so a suite that takes twenty minutes
 * is watchable while it runs. Metrics, extra parameters and the report document
 * are accumulated and folded into the end event, because those are only known
 * once it is over.
 */
export class EvaluationScope {
  #metrics: Record<string, number> = {};
  #params: Record<string, string> = {};
  #report: Record<string, unknown> | undefined;

  constructor(
    private readonly client: AiwatcherClient,
    private readonly context: Context,
    private readonly base: Record<string, unknown>,
  ) {}

  /**
   * One scored case. `reason` is what makes a score reviewable — a number with
   * no rationale is the thing people mean when they say they do not trust an
   * eval.
   */
  case(
    caseId: string,
    result: {
      passed?: boolean;
      score?: number;
      reason?: string;
      durationMs?: number;
      [key: string]: unknown;
    } = {},
  ): void {
    const { durationMs, ...rest } = result;
    this.client.emit('eval.case', this.context, {
      case_id: caseId,
      ...rest,
      ...(durationMs === undefined ? {} : { duration_ms: durationMs }),
    });
  }

  /** Aggregates. MLflow's `log_metrics`. */
  metrics(metrics: Record<string, number>): void {
    this.#metrics = { ...this.#metrics, ...metrics };
  }

  /** Anything held fixed. MLflow's `log_params`. */
  params(params: Record<string, unknown>): void {
    this.#params = { ...this.#params, ...stringify(params) };
  }

  /** The free-form half. MLflow's `log_dict`. */
  report(document: Record<string, unknown>): void {
    this.#report = document;
  }

  /** What the end event carries. */
  payload(): Record<string, unknown> {
    const inherited = (this.base.params as Record<string, string> | undefined) ?? {};
    return {
      ...(Object.keys(this.#metrics).length > 0 ? { metrics: this.#metrics } : {}),
      ...(Object.keys(this.#params).length > 0
        ? { params: { ...inherited, ...this.#params } }
        : {}),
      ...(this.#report === undefined ? {} : { report: this.#report }),
    };
  }
}

/**
 * Parameters are labels, so they arrive as strings.
 *
 * Bounded at the same 500 characters MLflow's own parameter limit uses: a
 * parameter that needs more than that is a report, and there is a field for
 * those.
 */
function stringify(params: Record<string, unknown>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(params).map(([key, value]) => [key, String(value).slice(0, 500)]),
  );
}

export interface WorkflowNode {
  id: string;
  name?: string;
  kind?: string;
  agent?: string;
}

export interface WorkflowEdge {
  from: string;
  to: string;
  label?: string;
}

export interface WorkflowOptions {
  /** `["a", "b"]` and `[{ id: "a" }]` are both graphs. */
  nodes?: Array<string | WorkflowNode>;
  /** `[["a", "b"]]` and `[{ from: "a", to: "b" }]` are both edge lists. */
  edges?: Array<[string, string] | WorkflowEdge>;
  name?: string;
  /** Joins several processes into one traversal. See `AiwatcherClient.workflow`. */
  executionId?: string;
  runId?: string;
  conversationId?: string;
}

export interface ArtifactOptions {
  uri: string;
  node?: string;
  mediaType?: string;
  sizeBytes?: number;
  digest?: string;
}

/** Accept the shortest thing somebody writes first. */
function normalizeNodes(nodes: WorkflowOptions['nodes']): WorkflowNode[] {
  return (nodes ?? []).flatMap((node) => {
    if (typeof node === 'string') return [{ id: node, name: node }];
    if (!node.id) return [];
    return [{ ...node, name: node.name ?? node.id }];
  });
}

function normalizeEdges(edges: WorkflowOptions['edges']): WorkflowEdge[] {
  return (edges ?? []).flatMap((edge) => {
    if (Array.isArray(edge)) {
      const [from, to] = edge;
      return from && to ? [{ from, to }] : [];
    }
    return edge.from && edge.to ? [edge] : [];
  });
}

/**
 * A content hash of the shape, so re-declaring costs nothing.
 *
 * Over the canonical form, not the caller's key order: a version that changed
 * because somebody reordered an option would make every execution look like a
 * new graph. Falls back to a length-based marker where `crypto.subtle` is
 * absent — the version only has to be *stable*, and a producer without WebCrypto
 * should not lose the whole declaration over it.
 */
async function topologyVersion(nodes: WorkflowNode[], edges: WorkflowEdge[]): Promise<string> {
  const canonical = JSON.stringify({
    edges: edges.map((edge) => ({ from: edge.from, label: edge.label, to: edge.to })),
    nodes: nodes.map((node) => ({ agent: node.agent, id: node.id, kind: node.kind, name: node.name })),
  });
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return `len:${canonical.length}`;
  const digest = await subtle.digest('SHA-256', new TextEncoder().encode(canonical));
  const hex = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  return `sha256:${hex.slice(0, 16)}`;
}

/** One traversal of a declared graph. */
export class WorkflowScope {
  constructor(
    private readonly client: AiwatcherClient,
    private readonly context: Context,
  ) {}

  /**
   * One stage. Becomes a span, and a node's status on the graph.
   *
   * `step.*` rather than an event type of its own, because a stage really is a
   * step: it has a start, an end and a duration, and it belongs in the
   * waterfall beside the LLM calls it makes.
   *
   * `attempt` distinguishes retries. Two attempts must carry different values
   * or they fold into one — the projection counts attempts by span key, which
   * derives from this.
   */
  async node<T>(
    nodeId: string,
    options: { agentId?: string; kind?: string; attempt?: string } | undefined,
    body: (node: NodeScope) => Promise<T>,
  ): Promise<T> {
    const context: Context = {
      ...this.context,
      causationId: this.context.causationId ?? this.context.correlationId,
      ...(options?.agentId ? { agentId: options.agentId } : {}),
    };
    const base = {
      node: nodeId,
      call_id: options?.attempt ?? newId(),
      step_type: options?.kind ?? 'chain',
    };
    const started = performance.now();
    this.client.emit('step.started', context, base);
    try {
      const result = await body(new NodeScope(this.client, context, nodeId));
      this.client.emit('step.completed', context, {
        ...base,
        duration_ms: performance.now() - started,
      });
      return result;
    } catch (error) {
      this.client.emit('step.failed', context, {
        ...base,
        error: String(error),
        duration_ms: performance.now() - started,
      });
      throw error;
    }
  }

  /**
   * An agent inside this traversal, outside any one stage.
   *
   * For an agent that coordinates rather than executes. One doing a *stage's*
   * work belongs under `NodeScope.agent`, so its span nests inside that stage.
   */
  async agent<T>(agentId: string, body: (agent: AgentScope) => Promise<T>): Promise<T> {
    return new RunScope(this.client, this.context).agent(agentId, body);
  }

  /** Record something this traversal produced, by reference. */
  artifact(name: string, options: ArtifactOptions): void {
    emitArtifact(this.client, this.context, name, options);
  }
}

/** One stage of a traversal, while it runs. */
export class NodeScope {
  constructor(
    private readonly client: AiwatcherClient,
    private readonly context: Context,
    private readonly nodeId: string,
  ) {}

  /** An agent doing this stage's work. Nests under the stage's span. */
  async agent<T>(agentId: string, body: (agent: AgentScope) => Promise<T>): Promise<T> {
    return new RunScope(this.client, this.context).agent(agentId, body);
  }

  /**
   * Record something this stage produced, **by reference**.
   *
   * The bytes stay where you put them. aiwatcher keeps the pointer because a
   * pointer is bounded and a floor-plan PDF is not; an artifact with no `uri`
   * is dropped rather than listed as a row nobody can open.
   */
  artifact(name: string, options: Omit<ArtifactOptions, 'node'>): void {
    emitArtifact(this.client, this.context, name, { ...options, node: this.nodeId });
  }
}

function emitArtifact(
  client: AiwatcherClient,
  context: Context,
  name: string,
  options: ArtifactOptions,
): void {
  client.emit('artifact.produced', context, {
    name,
    uri: options.uri,
    ...(options.node ? { node: options.node } : {}),
    ...(options.mediaType ? { media_type: options.mediaType } : {}),
    ...(options.sizeBytes !== undefined ? { size_bytes: options.sizeBytes } : {}),
    ...(options.digest ? { digest: options.digest } : {}),
  });
}

export class RunScope {
  constructor(
    private readonly client: AiwatcherClient,
    private readonly context: Context,
  ) {}

  async agent<T>(agentId: string, body: (agent: AgentScope) => Promise<T>): Promise<T> {
    const context: Context = {
      ...this.context,
      agentId,
      // Emmett's rule: an unseeded causation roots itself on the correlation.
      causationId: this.context.causationId ?? this.context.correlationId,
    };
    this.client.emit('agent.started', context);
    try {
      const result = await body(new AgentScope(this.client, context));
      this.client.emit('agent.completed', context);
      return result;
    } catch (error) {
      this.client.emit('agent.failed', context, { error: String(error) });
      throw error;
    }
  }
}

export interface LlmRequest {
  model: string;
  provider?: string;
  /**
   * Distinguishes concurrent calls inside one agent. Generated when omitted,
   * but pass your provider's request id where you have one — it makes the span
   * joinable with the provider's own logs.
   */
  callId?: string;
  [key: string]: unknown;
}

export interface Usage {
  prompt_tokens?: number;
  completion_tokens?: number;
  cached_tokens?: number;
  finish_reason?: string;
  [key: string]: unknown;
}

export class AgentScope {
  constructor(
    private readonly client: AiwatcherClient,
    private readonly context: Context,
  ) {}

  /**
   * Record this agent addressing another one.
   *
   * The one thing nothing else here can see. A trace records nesting — agent
   * calls LLM calls tool — and two agents exchanging work through a queue, a
   * file or a task graph nest inside nothing at all. Without this the question
   * "do these agents actually talk" has no answer in the data.
   *
   * A point, not a scope: a handoff has a moment, not a duration. It becomes a
   * span event on this agent's span, and an edge on the graph.
   */
  message(
    to: string,
    options?: { kind?: string; channel?: string } & Record<string, unknown>,
  ): void {
    const { kind, channel, ...extra } = options ?? {};
    this.client.emit('agent.message', this.context, {
      to,
      kind: kind ?? 'handoff',
      ...extra,
      ...(this.context.agentId ? { from: this.context.agentId } : {}),
      ...(channel ? { channel } : {}),
    });
  }

  async llm<T>(request: LlmRequest, body: (call: LlmCall) => Promise<T>): Promise<T> {
    const { callId, ...rest } = request;
    const base = { call_id: callId ?? newId(), ...rest };
    const started = performance.now();
    this.client.emit('llm.started', this.context, base);
    const call = new LlmCall(this.client, this.context, base);
    try {
      const result = await body(call);
      this.client.emit('llm.completed', this.context, {
        ...base,
        ...call.usageData,
        duration_ms: performance.now() - started,
      });
      return result;
    } catch (error) {
      this.client.emit('llm.failed', this.context, {
        ...base,
        error: String(error),
        duration_ms: performance.now() - started,
      });
      throw error;
    }
  }

  async tool<T>(
    name: string,
    args: Record<string, unknown> & { callId?: string },
    body: () => Promise<T>,
  ): Promise<T> {
    const { callId, ...rest } = args;
    const base = { call_id: callId ?? newId(), tool_name: name, ...rest };
    const started = performance.now();
    this.client.emit('tool.started', this.context, base);
    try {
      const result = await body();
      this.client.emit('tool.completed', this.context, {
        ...base,
        duration_ms: performance.now() - started,
      });
      return result;
    } catch (error) {
      this.client.emit('tool.failed', this.context, {
        ...base,
        error: String(error),
        duration_ms: performance.now() - started,
      });
      throw error;
    }
  }
}

export class LlmCall {
  usageData: Usage = {};

  constructor(
    private readonly client: AiwatcherClient,
    private readonly context: Context,
    private readonly base: Record<string, unknown>,
  ) {}

  /** Call once, when the first token arrives. Drives time-to-first-token. */
  firstToken(): void {
    this.client.emit('llm.first_token', this.context, this.base);
  }

  /**
   * A streamed fragment.
   *
   * Reaches the live panel and the durable log; it does **not** become a trace
   * record. Streaming a 2000-token response emits 2000 of these for one LLM
   * call, and storing them as spans would swamp the trace store for nothing.
   */
  chunk(text: string): void {
    this.client.emit('llm.chunk', this.context, { ...this.base, text });
  }

  /** Record the outcome. Folded into `llm.completed` when the scope resolves. */
  usage(usage: Usage): void {
    this.usageData = { ...this.usageData, ...usage };
  }
}
