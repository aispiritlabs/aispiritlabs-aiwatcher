/**
 * What a span already says about itself.
 *
 * The assembler writes the model a call ran on, the settings that changed its
 * answer, what it spent and which registered prompt it named as span
 * attributes (ADR_0003, `aiwatcher_core::attrs`). All of that travels in
 * `GET /api/v1/runs/{id}`, and until this file none of it was drawn: the
 * waterfall read one attribute and everything else was left to whoever thought
 * to expand a payload in the event feed.
 *
 * Two rules keep the reading honest. A fact is lifted only where its attribute
 * is actually there — an absent temperature is absent, never `0`. And nothing
 * is dropped: [`otherAttributes`] is every attribute this file did not name, so
 * an attribute a producer invents is shown without a release here.
 */

/** A span as `GET /api/v1/runs/{id}` returns one. */
export interface Span {
  trace_id: string;
  span_id: string;
  parent_span_id?: string | null;
  name: string;
  kind: string;
  start: string;
  end: string;
  status: { status: string; message?: string };
  attributes: Array<[string, unknown]>;
  events?: Array<{ name: string; at: string; attributes?: Array<[string, unknown]> }>;
  links?: Array<{ trace_id: string; span_id: string }>;
}

/**
 * Which family a span belongs to.
 *
 * Read from the attributes where possible rather than parsed out of the name: a
 * step names itself after what it is (`knowledge_base`), so its name says
 * nothing about the family it belongs to.
 */
export type SpanFamily = 'run' | 'agent' | 'llm' | 'tool' | 'step';

/** One label and one already-formatted value. */
export interface Fact {
  label: string;
  value: string;
  /** Rendered as a copyable id rather than as prose. */
  id?: boolean;
}

export interface FactGroup {
  title: string;
  facts: Fact[];
}

/**
 * The registry version of the model that served a call.
 *
 * Read beside the prompt rather than among the request settings, where it sat
 * first: a temperature is something the caller chose, and this is what
 * answered. Both are references into a registry that outlives the log, which
 * is what makes them the two halves of a call's lineage.
 */
export interface ModelReference {
  /** `gen_ai.request.model` — the registry's name where one served the call. */
  name?: string;
  /** `aiwatcher.model.version`, which is a digest only from the serving profile. */
  version: string;
}

/** The registered prompt a call ran on — a reference, never the text. */
export interface PromptReference {
  name?: string;
  versionId?: string;
  /** Whether a host that saw the request found the template in it. */
  verified?: boolean;
  /** Whether it found the request to be nothing but that template rendered. */
  exact?: boolean;
}

export interface TokenUsage {
  input: number;
  output: number;
  cached: number;
}

export interface SpanFacts {
  family: SpanFamily;
  /** The model or the tool: what the row is about, past its span name. */
  headline?: string;
  tokens?: TokenUsage;
  /**
   * What the provider said this call cost, in US dollars. Absent where it said
   * nothing — an unpriced call cost something nobody stated, not nothing.
   */
  costUsd?: number;
  prompt?: PromptReference;
  model?: ModelReference;
  groups: FactGroup[];
}

const GENAI = {
  operation: 'gen_ai.operation.name',
  provider: 'gen_ai.provider.name',
  system: 'gen_ai.system',
  requestModel: 'gen_ai.request.model',
  responseModel: 'gen_ai.response.model',
  responseId: 'gen_ai.response.id',
  finishReasons: 'gen_ai.response.finish_reasons',
  inputTokens: 'gen_ai.usage.input_tokens',
  outputTokens: 'gen_ai.usage.output_tokens',
  cachedTokens: 'gen_ai.usage.cached_tokens',
  agent: 'gen_ai.agent.id',
  agentName: 'gen_ai.agent.name',
  conversation: 'gen_ai.conversation.id',
  tool: 'gen_ai.tool.name',
  toolCall: 'gen_ai.tool.call.id',
} as const;

const OWN = {
  stepType: 'aiwatcher.span.step_type',
  stepName: 'aiwatcher.span.step_name',
  closedBy: 'aiwatcher.span.closed_by',
  chunkCount: 'aiwatcher.span.chunk_count',
  promptName: 'aiwatcher.prompt.name',
  promptVersion: 'aiwatcher.prompt.version_id',
  promptVerified: 'aiwatcher.prompt.verified',
  promptExact: 'aiwatcher.prompt.exact',
  variant: 'aiwatcher.variant.id',
  modelVersion: 'aiwatcher.model.version',
  service: 'aiwatcher.source.service',
  instance: 'aiwatcher.source.instance',
  sdk: 'aiwatcher.source.sdk',
  publishedBy: 'aiwatcher.source.published_by',
  costUsd: 'aiwatcher.usage.cost_usd',
  documentCount: 'aiwatcher.step.document_count',
  topK: 'aiwatcher.step.top_k',
  candidateCount: 'aiwatcher.step.candidate_count',
  score: 'aiwatcher.step.score',
} as const;

/** The request settings, in the order a person compares two runs in. */
const SETTINGS: Array<[string, string]> = [
  ['gen_ai.request.temperature', 'Temperature'],
  ['gen_ai.request.max_tokens', 'Max tokens'],
  ['gen_ai.request.top_p', 'Top p'],
  ['gen_ai.request.top_k', 'Top k'],
  ['gen_ai.request.seed', 'Seed'],
  ['gen_ai.request.stop_sequences', 'Stop'],
  ['gen_ai.request.frequency_penalty', 'Frequency penalty'],
  ['gen_ai.request.presence_penalty', 'Presence penalty'],
];

export function attributesOf(span: Span): Map<string, unknown> {
  return new Map(span.attributes ?? []);
}

/** `retriever`, `embedding`, `guardrail`… when this span is a step. */
export function stepTypeOf(span: Span): string | undefined {
  return text(attributesOf(span).get(OWN.stepType));
}

export function familyOf(span: Span): SpanFamily {
  if (stepTypeOf(span)) return 'step';
  if (span.name === 'run') return 'run';
  if (span.name.startsWith('invoke_agent')) return 'agent';
  if (span.name.startsWith('execute_tool')) return 'tool';
  return 'llm';
}

export function factsOf(span: Span): SpanFacts {
  const held = attributesOf(span);
  const family = familyOf(span);
  const groups: FactGroup[] = [];

  const call: Fact[] = [];
  push(call, 'Provider', held.get(GENAI.provider) ?? held.get(GENAI.system));
  push(call, 'Model', held.get(GENAI.requestModel));
  // Only where it differs: a provider that served exactly what was asked for
  // says so by repeating the name, and a repeated row reads as two facts.
  const responseModel = text(held.get(GENAI.responseModel));
  if (responseModel && responseModel !== text(held.get(GENAI.requestModel))) {
    call.push({ label: 'Answered by', value: responseModel });
  }
  push(call, 'Response id', held.get(GENAI.responseId), { id: true });
  push(call, 'Finish reason', held.get(GENAI.finishReasons));
  push(call, 'Tool', held.get(GENAI.tool));
  push(call, 'Call id', held.get(GENAI.toolCall), { id: true });
  push(call, 'Step', held.get(OWN.stepType));
  push(call, 'Name', held.get(OWN.stepName));
  push(call, 'Documents', held.get(OWN.documentCount));
  push(call, 'Top k', held.get(OWN.topK));
  push(call, 'Candidates', held.get(OWN.candidateCount));
  push(call, 'Score', held.get(OWN.score));
  if (call.length > 0) groups.push({ title: 'Call', facts: call });

  const settings: Fact[] = [];
  for (const [key, label] of SETTINGS) push(settings, label, held.get(key));
  if (settings.length > 0) groups.push({ title: 'Settings', facts: settings });

  const identity: Fact[] = [];
  push(identity, 'Agent', held.get(GENAI.agentName) ?? held.get(GENAI.agent));
  push(identity, 'Session', held.get(GENAI.conversation), { id: true });
  push(identity, 'Variant', held.get(OWN.variant), { id: true });
  push(identity, 'Runtime', held.get(OWN.service));
  push(identity, 'Instance', held.get(OWN.instance));
  push(identity, 'SDK', held.get(OWN.sdk));
  push(identity, 'Published by', held.get(OWN.publishedBy));
  if (identity.length > 0) groups.push({ title: 'Identity', facts: identity });

  // How a span was closed is the difference between a producer that reported
  // its end and one the orphan sweeper had to finish for it, which is worth
  // reading before believing a duration.
  const assembly: Fact[] = [];
  push(assembly, 'Closed by', held.get(OWN.closedBy));
  push(assembly, 'Chunks', held.get(OWN.chunkCount));
  if (assembly.length > 0) groups.push({ title: 'Assembly', facts: assembly });

  return {
    family,
    headline: text(held.get(GENAI.requestModel) ?? held.get(GENAI.tool) ?? held.get(OWN.stepName)),
    tokens: tokensOf(held),
    costUsd: count(held.get(OWN.costUsd)),
    prompt: promptOf(held),
    model: modelOf(held),
    groups,
  };
}

function tokensOf(held: Map<string, unknown>): TokenUsage | undefined {
  const input = count(held.get(GENAI.inputTokens));
  const output = count(held.get(GENAI.outputTokens));
  const cached = count(held.get(GENAI.cachedTokens));
  if (input === undefined && output === undefined && cached === undefined) return undefined;
  return { input: input ?? 0, output: output ?? 0, cached: cached ?? 0 };
}

function modelOf(held: Map<string, unknown>): ModelReference | undefined {
  const version = text(held.get(OWN.modelVersion));
  if (!version) return undefined;
  return { name: text(held.get(GENAI.requestModel)), version };
}

function promptOf(held: Map<string, unknown>): PromptReference | undefined {
  const name = text(held.get(OWN.promptName));
  const versionId = text(held.get(OWN.promptVersion));
  if (!name && !versionId) return undefined;
  return {
    name,
    versionId,
    verified: flag(held.get(OWN.promptVerified)),
    exact: flag(held.get(OWN.promptExact)),
  };
}

/** Every key [`factsOf`] renders, so the attribute table can leave them out. */
const NAMED: ReadonlySet<string> = new Set<string>([
  ...Object.values(GENAI),
  ...Object.values(OWN),
  ...SETTINGS.map(([key]) => key),
]);

/**
 * The attributes nothing above drew.
 *
 * The ids that identify the event on the log — `messaging.*`,
 * `aiwatcher.stream.*`, `aiwatcher.run.id` — are correlation, not description:
 * they are what a reader pastes into another system, and they belong with the
 * raw events rather than beside a model's settings. They are still here, behind
 * `everything`, because a view whose promise is that nothing is hidden cannot
 * have a list it quietly filters.
 */
export function otherAttributes(span: Span, everything = false): Array<[string, string]> {
  return (span.attributes ?? [])
    .filter(([key]) => everything || (!NAMED.has(key) && !isCorrelation(key)))
    .map(([key, value]) => [key, format(value)] as [string, string])
    .sort(([left], [right]) => left.localeCompare(right));
}

function isCorrelation(key: string): boolean {
  return (
    key.startsWith('messaging.') ||
    key.startsWith('aiwatcher.stream.') ||
    key.startsWith('aiwatcher.event.') ||
    key === 'aiwatcher.run.id'
  );
}

function push(into: Fact[], label: string, value: unknown, options: { id?: boolean } = {}): void {
  const shown = format(value);
  if (shown === '') return;
  into.push({ label, value: shown, id: options.id });
}

export function format(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (Array.isArray(value)) return value.map(format).filter(Boolean).join(', ');
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value);
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

function count(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function flag(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined;
}
