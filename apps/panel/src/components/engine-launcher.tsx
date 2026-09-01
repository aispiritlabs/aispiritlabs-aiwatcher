import * as React from 'react';
import { Link } from '@tanstack/react-router';
import { useMutation, useQuery } from '@tanstack/react-query';
import { ExternalLink, Radio, Rocket, Search, Workflow } from 'lucide-react';

import {
  describeEngine,
  getLaunch,
  listEngineWorkflows,
  launchWorkflow,
} from '@/api/generated/sdk.gen';
import type {
  EngineParameter,
  EngineWorkflow,
  LaunchAccepted,
  PipelineStage,
} from '@/api/generated/types.gen';
import { needsRole, useCan } from '@/lib/auth';
import { Badge, Button, Card, EmptyState, IdChip, Spinner } from '@/components/ui/primitives';

/**
 * Starting a workflow the orchestrator already holds.
 *
 * The one control in this panel that reaches outside aiwatcher, and the whole
 * design is about keeping that visible. It does not describe work: it lists
 * what the engine has, renders the inputs that thing itself declared, and
 * sends values for them. There is no field for an endpoint, an image or a
 * command, because there is no such field in the API — see ADR_0016.
 *
 * Used from Data Curation and from Experiments with a different `stage`, which
 * is why it is a component rather than a page: the feature/training/inference
 * cycle is four questions asked in four places, and only the prefill differs.
 *
 * ## Why the prefill is the feature
 *
 * A curation workflow's inputs are almost always the same four things — what
 * to produce, where to read it from, which rows, and over what period — and
 * the page asking for the launch already knows three of them. `suggest` maps
 * a declared parameter name onto the context the page is showing, so the
 * common case is picking a workflow and pressing the button. It is a guess by
 * name and it is always visible and editable; a prefill that could not be
 * seen would be a launch nobody chose.
 */

/** What the page around the launcher already knows. */
export type LaunchContext = {
  /** The dataset this page is about — the "what". */
  dataset?: string;
  /** Where rows come from, when the page has one. */
  source?: string;
  /** The window the page is showing, in seconds. `0` or absent means all. */
  windowSeconds?: number;
  /** Anything else, keyed by exact parameter name. Wins over every guess. */
  values?: Record<string, unknown>;
};

export function isEngineDisabled(error: unknown): boolean {
  const body = error as { code?: string } | null | undefined;
  return body?.code === 'engine_disabled';
}

function apiError(error: unknown, fallback: string): Error {
  if (
    error &&
    typeof error === 'object' &&
    'message' in error &&
    typeof error.message === 'string'
  ) {
    return new Error(error.message);
  }
  return new Error(fallback);
}

const ISO = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, 'Z');

/**
 * A declared parameter, and what this page would put in it.
 *
 * Matched on the name, in the vocabulary orchestrated pipelines actually use.
 * A name nothing matches falls back to the engine's own default, and a
 * parameter with neither is left empty for somebody to fill in.
 */
function suggest(parameter: EngineParameter, context: LaunchContext): unknown {
  const exact = context.values?.[parameter.name];
  if (exact !== undefined) return exact;

  const name = parameter.name.toLowerCase();
  const seconds = context.windowSeconds ?? 0;
  const now = new Date();

  if (parameter.kind === 'datetime') {
    if (/(^|_)(since|start|from|after|begin)/.test(name)) {
      return seconds ? ISO(new Date(now.getTime() - seconds * 1000)) : undefined;
    }
    if (/(^|_)(until|end|to|before)/.test(name)) return ISO(now);
  }
  if (
    parameter.kind === 'duration' &&
    /(window|period|lookback|range|interval|duration)/.test(name)
  ) {
    return seconds ? `${seconds}s` : undefined;
  }
  if (parameter.kind === 'integer' && /(window|period|lookback)_?seconds/.test(name)) {
    return seconds || undefined;
  }
  if (context.dataset && /(dataset|output|target|destination|sink|table)/.test(name)) {
    return context.dataset;
  }
  if (context.source && /(source|input|origin|from_dataset)/.test(name)) return context.source;

  return parameter.default ?? undefined;
}

/** What the form holds: one string per field, parsed on submit. */
type Draft = Record<string, string>;

function draftFor(workflow: EngineWorkflow, context: LaunchContext): Draft {
  const draft: Draft = {};
  for (const parameter of workflow.parameters) {
    const suggested = suggest(parameter, context);
    draft[parameter.name] =
      suggested === undefined || suggested === null
        ? ''
        : typeof suggested === 'string'
          ? suggested
          : JSON.stringify(suggested);
  }
  return draft;
}

/**
 * The form's strings, as the JSON the API binds to the engine's own types.
 *
 * Blank optional fields are dropped rather than sent empty: an empty string
 * would override the launch plan's own default, which is the opposite of
 * leaving a field alone.
 */
function readDraft(workflow: EngineWorkflow, draft: Draft): Record<string, unknown> {
  const inputs: Record<string, unknown> = {};
  for (const parameter of workflow.parameters) {
    const raw = (draft[parameter.name] ?? '').trim();
    if (!raw) {
      if (parameter.required) {
        throw new Error(`${parameter.name} is required.`);
      }
      continue;
    }
    if (parameter.kind === 'collection' || parameter.kind === 'map' || parameter.kind === 'json') {
      try {
        inputs[parameter.name] = JSON.parse(raw);
      } catch {
        throw new Error(`${parameter.name} has to be JSON — ${raw.slice(0, 40)}`);
      }
      continue;
    }
    if (parameter.kind === 'integer' || parameter.kind === 'float') {
      const parsed = Number(raw);
      if (Number.isNaN(parsed)) throw new Error(`${parameter.name} has to be a number.`);
      inputs[parameter.name] = parsed;
      continue;
    }
    if (parameter.kind === 'boolean') {
      inputs[parameter.name] = raw === 'true';
      continue;
    }
    inputs[parameter.name] = raw;
  }
  return inputs;
}

export function EngineLauncher({
  stage,
  title,
  summary,
  context = {},
  search,
  onSearchChange,
  selected,
  onSelect,
}: {
  /** Narrows the catalog to what the engine guessed this stage from a name. */
  stage?: PipelineStage;
  title: string;
  summary: string;
  context?: LaunchContext;
  /** In the URL, like every other filter. */
  search: string;
  onSearchChange: (search: string) => void;
  selected?: string;
  onSelect: (id: string | undefined) => void;
}) {
  const mayLaunch = useCan('admin');
  const [draft, setDraft] = React.useState<Draft>({});
  const [find, setFind] = React.useState(search);
  const [accepted, setAccepted] = React.useState<LaunchAccepted | null>(null);

  // The URL is the state and the input is a draft of it — the same 250 ms
  // debounce every other search box here uses.
  React.useEffect(() => setFind(search), [search]);
  React.useEffect(() => {
    if (find === search) return;
    const timer = setTimeout(() => onSearchChange(find), 250);
    return () => clearTimeout(timer);
  }, [find, search, onSearchChange]);

  const engine = useQuery({
    queryKey: ['engine'],
    queryFn: async () => {
      const response = await describeEngine();
      if (!response.data) throw response.error ?? new Error('No engine.');
      return response.data;
    },
    retry: false,
  });

  const catalog = useQuery({
    queryKey: ['engine', 'workflows', stage ?? null, search],
    enabled: engine.isSuccess,
    queryFn: async () => {
      const response = await listEngineWorkflows({
        query: { stage, search: search || undefined, limit: 25 },
      });
      if (!response.data) throw apiError(response.error, 'The engine catalog could not be read.');
      return response.data.workflows;
    },
  });

  const workflow = catalog.data?.find((candidate) => candidate.id === selected);

  // The form follows the selection, and re-follows it when the page's own
  // context changes underneath — a different window means a different range.
  const contextKey = JSON.stringify(context);
  React.useEffect(() => {
    if (!workflow) return;
    setDraft(draftFor(workflow, context));
    setAccepted(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workflow?.id, contextKey]);

  const launch = useMutation({
    mutationFn: async () => {
      if (!workflow) throw new Error('Pick a workflow first.');
      const inputs = readDraft(workflow, draft);
      const response = await launchWorkflow({ body: { workflow: workflow.id, inputs } });
      if (!response.data) throw apiError(response.error, 'The engine would not take the launch.');
      return response.data;
    },
    onSuccess: setAccepted,
  });

  if (engine.isPending) return null;
  if (engine.isError) {
    return isEngineDisabled(engine.error) ? <EngineDisabled title={title} /> : null;
  }

  return (
    <Card className="overflow-hidden">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-border p-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Workflow className="h-3.5 w-3.5 text-primary" />
            <span className="text-sm font-semibold">{title}</span>
            <Badge>{engine.data.kind}</Badge>
            <Badge>
              {engine.data.project}/{engine.data.domain}
            </Badge>
          </div>
          <p className="mt-1 max-w-3xl text-xs text-muted-foreground">{summary}</p>
        </div>
        <label className="flex h-9 items-center gap-2 rounded-md border border-border px-2">
          <Search className="h-3.5 w-3.5 text-muted-foreground" />
          <input
            value={find}
            onChange={(event) => setFind(event.target.value)}
            placeholder="Find a workflow"
            className="w-44 bg-transparent text-sm outline-none"
          />
        </label>
      </div>

      <div className="grid gap-0 lg:grid-cols-[18rem_minmax(0,1fr)]">
        <div className="max-h-80 overflow-y-auto border-b border-border lg:border-r lg:border-b-0">
          {catalog.isPending ? (
            <div className="p-3">
              <Spinner />
            </div>
          ) : catalog.isError ? (
            <p className="p-3 text-xs text-danger">{catalog.error.message}</p>
          ) : catalog.data.length ? (
            <div className="divide-y divide-border/50">
              {catalog.data.map((candidate) => (
                <button
                  key={candidate.id}
                  type="button"
                  onClick={() => onSelect(candidate.id === selected ? undefined : candidate.id)}
                  className={`w-full p-3 text-left hover:bg-accent/40 ${
                    candidate.id === selected ? 'bg-accent/60' : ''
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-sm font-medium">{candidate.name}</span>
                    {candidate.active ? null : <Badge tone="warning">inactive</Badge>}
                  </div>
                  <p className="mt-0.5 truncate text-[11px] text-muted-foreground">
                    {candidate.description || 'No description registered.'}
                  </p>
                  <p className="mt-0.5 text-[11px] text-muted-foreground">
                    <code className="id">{candidate.version || 'unversioned'}</code>
                    {candidate.stage_hint ? ` · ${candidate.stage_hint}` : null}
                    {` · ${candidate.parameters.length} inputs`}
                  </p>
                </button>
              ))}
            </div>
          ) : (
            <div className="p-3">
              <EmptyState
                title="Nothing registered here"
                hint={`No launch plan in ${engine.data.project}/${engine.data.domain} matched.`}
              />
            </div>
          )}
        </div>

        <div className="min-w-0 p-3">
          {workflow ? (
            <form
              className="flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                launch.mutate();
              }}
            >
              <div className="flex flex-wrap items-center gap-2">
                <IdChip label="workflow" value={workflow.id} />
                {workflow.url ? (
                  <a
                    href={workflow.url}
                    target="_blank"
                    rel="noreferrer"
                    className="flex items-center gap-1 text-xs text-primary hover:underline"
                  >
                    <ExternalLink className="h-3 w-3" /> in {engine.data.kind}
                  </a>
                ) : null}
              </div>

              {workflow.parameters.length ? (
                <div className="grid gap-3 sm:grid-cols-2">
                  {workflow.parameters.map((parameter) => (
                    <ParameterField
                      key={parameter.name}
                      parameter={parameter}
                      value={draft[parameter.name] ?? ''}
                      onChange={(value) =>
                        setDraft((previous) => ({ ...previous, [parameter.name]: value }))
                      }
                    />
                  ))}
                </div>
              ) : (
                <p className="text-xs text-muted-foreground">
                  This workflow declares no inputs. There is nothing to set.
                </p>
              )}

              <div className="flex flex-wrap items-center gap-2">
                <Button type="submit" disabled={!mayLaunch || launch.isPending}>
                  {launch.isPending ? <Spinner /> : <Rocket className="h-3.5 w-3.5" />} Launch
                </Button>
                {mayLaunch ? null : (
                  <span className="text-xs text-muted-foreground">{needsRole('admin')}</span>
                )}
              </div>

              {launch.error ? <p className="text-xs text-danger">{launch.error.message}</p> : null}
              {accepted ? <Accepted accepted={accepted} /> : null}
            </form>
          ) : (
            <EmptyState
              title="Pick a workflow"
              hint="Its declared inputs become this form: what to produce, where to read, which rows, over what period."
            />
          )}
        </div>
      </div>
    </Card>
  );
}

/**
 * One declared input, as the control its type deserves.
 *
 * A date-time control rather than a text box is most of what makes "set the
 * range" a five-second job, and a select rather than a text box is the
 * difference between a filter somebody picks and one they misspell.
 */
function ParameterField({
  parameter,
  value,
  onChange,
}: {
  parameter: EngineParameter;
  value: string;
  onChange: (value: string) => void;
}) {
  const label = (
    <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
      {parameter.name}
      {parameter.required ? <span className="text-danger">*</span> : null}
      {parameter.type_name ? <code className="id text-[10px]">{parameter.type_name}</code> : null}
    </span>
  );
  const input =
    'h-9 w-full rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary';

  const control = () => {
    switch (parameter.kind) {
      case 'datetime': {
        // The control speaks local time and the engine speaks RFC 3339, so the
        // conversion happens here rather than in anybody's head.
        const local = value ? toLocalInput(value) : '';
        return (
          <input
            type="datetime-local"
            value={local}
            onChange={(event) =>
              onChange(event.target.value ? new Date(event.target.value).toISOString() : '')
            }
            className={input}
          />
        );
      }
      case 'boolean':
        return (
          <select
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className={input}
          >
            <option value="">unset</option>
            <option value="true">true</option>
            <option value="false">false</option>
          </select>
        );
      case 'enum':
        return (
          <select
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className={input}
          >
            <option value="">unset</option>
            {parameter.enum_values?.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        );
      case 'integer':
      case 'float':
        return (
          <input
            type="number"
            step={parameter.kind === 'float' ? 'any' : 1}
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className={input}
          />
        );
      case 'collection':
      case 'map':
      case 'json':
        return (
          <textarea
            value={value}
            onChange={(event) => onChange(event.target.value)}
            rows={2}
            spellCheck={false}
            placeholder={parameter.kind === 'collection' ? '["a", "b"]' : '{"key": "value"}'}
            className="id w-full resize-y rounded-md border border-border bg-transparent p-2 outline-none focus-visible:ring-2 focus-visible:ring-primary"
          />
        );
      default:
        return (
          <input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder={parameter.kind === 'duration' ? '24h, 900s' : undefined}
            className={input}
          />
        );
    }
  };

  return (
    <label className="flex flex-col gap-1">
      {label}
      {control()}
      {parameter.description ? (
        <span className="text-[11px] text-muted-foreground">{parameter.description}</span>
      ) : null}
    </label>
  );
}

/** RFC 3339 into what `datetime-local` accepts, in the reader's own zone. */
function toLocalInput(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 16);
}

/**
 * The acknowledgement, and the two places it leads.
 *
 * Nothing has run yet, so this shows an engine reference and a phase polled
 * from the engine — never a result. The evidence that the work happened is the
 * events it publishes, which is why the second link is aiwatcher's own live
 * view of the execution rather than the orchestrator's console.
 */
function Accepted({ accepted }: { accepted: LaunchAccepted }) {
  const status = useQuery({
    queryKey: ['engine', 'launch', accepted.reference],
    queryFn: async () => {
      const response = await getLaunch({ path: { reference: accepted.reference } });
      if (!response.data) throw apiError(response.error, 'The engine lost the execution.');
      return response.data;
    },
    // Until it stops moving. A finished execution is not worth asking about
    // again, and an engine is not a metrics store.
    refetchInterval: (query) =>
      query.state.data && ['queued', 'running'].includes(query.state.data.phase) ? 5_000 : false,
  });

  return (
    <div className="flex flex-col gap-2 rounded-md border border-success/40 p-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <span>Accepted as</span>
        <code className="id">{accepted.reference.split(':').pop()}</code>
        {status.data ? <Badge tone={toneFor(status.data.phase)}>{status.data.phase}</Badge> : null}
      </div>
      {status.data?.message ? <p className="text-xs text-danger">{status.data.message}</p> : null}
      <div className="flex flex-wrap items-center gap-3 text-xs">
        {accepted.workflow_run_id ? (
          <Link
            to="/workflows"
            search={{ execution: accepted.workflow_run_id }}
            className="flex items-center gap-1 text-primary hover:underline"
          >
            <Radio className="h-3 w-3" /> Watch it here
          </Link>
        ) : null}
        {accepted.url ? (
          <a
            href={accepted.url}
            target="_blank"
            rel="noreferrer"
            className="flex items-center gap-1 text-primary hover:underline"
          >
            <ExternalLink className="h-3 w-3" /> Open in the orchestrator
          </a>
        ) : null}
      </div>
      <p className="text-[11px] text-muted-foreground">
        Nothing has run yet. This execution appears in Workflows once its producer publishes under{' '}
        <code className="id">{accepted.workflow_run_id ?? 'its own id'}</code>.
      </p>
    </div>
  );
}

function toneFor(phase: string): 'success' | 'danger' | 'warning' | undefined {
  if (phase === 'succeeded') return 'success';
  if (phase === 'failed' || phase === 'aborted') return 'danger';
  if (phase === 'unknown') return 'warning';
  return undefined;
}

/**
 * What an area shows when the instance has no orchestrator.
 *
 * The API answers 501 rather than 404, and the difference is why this exists:
 * an empty catalog would say "nothing is registered", which is a different
 * problem with a different fix. This says which variable is unset.
 */
export function EngineDisabled({ title }: { title: string }) {
  return (
    <Card className="border-dashed p-3">
      <p className="text-sm font-medium">{title}</p>
      <p className="mt-1 max-w-3xl text-xs leading-relaxed text-muted-foreground">
        This instance has no pipeline engine configured, so there is nothing to list and nothing to
        start. Set <code>AIWATCHER_ENGINE=flyte</code> and <code>AIWATCHER_FLYTE_ENDPOINT</code> —
        with <code>AIWATCHER_FLYTE_PROJECT</code> and <code>AIWATCHER_FLYTE_DOMAIN</code> for the
        namespace to browse. Everything else on this page works without one.
      </p>
    </Card>
  );
}
