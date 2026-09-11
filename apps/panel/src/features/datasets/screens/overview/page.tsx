import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getRouteApi, Link } from '@tanstack/react-router';
import { Database, ExternalLink, Globe, Play, Sparkles, Users } from 'lucide-react';
import * as React from 'react';

import {
  listConversations,
  listDatasets,
  listDimension,
  publishDataset,
} from '@/api/generated/sdk.gen';
import type { DatasetSummary, DimensionKind } from '@/api/generated/types.gen';
import { HubDiscovery } from '@/shared/components/hub-discovery';
import { DatasetExplorer, type DatasetView } from '@/features/datasets/components/dataset-explorer';
import { FlowResultView } from '@/shared/components/flow-preview';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/shared/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import {
  ENGINE_LABEL,
  isQueryEngineAvailable,
  runQuery,
  simulateQuery,
  useQueryEngine,
  type QueryEngineName,
} from '@/shared/lib/query';
import { answerOf } from '@/shared/lib/result';
import { cn } from '@/shared/lib/utils';

const routeApi = getRouteApi('/datasets');

type PromotionScope = 'session' | 'agent' | 'agents';

export function DatasetsPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const queryClient = useQueryClient();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const [scope, setScope] = React.useState<PromotionScope>('session');
  const [session, setSession] = React.useState('');
  const [agent, setAgent] = React.useState('');
  const [agents, setAgents] = React.useState<string[]>([]);
  const [datasetName, setDatasetName] = React.useState('evaluation/promoted-conversations');

  const available = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isQueryEngineAvailable,
    refetchInterval: 10_000,
  });
  // The language the promotion is generated in: the deployed engine's (AW-3).
  const engine = useQueryEngine().data?.engine ?? 'flow';
  const catalog = useQuery({
    queryKey: ['datasets'],
    queryFn: async () => {
      const response = await listDatasets();
      return answerOf(response, 'Could not load datasets.').datasets;
    },
  });
  const conversations = useQuery({
    queryKey: ['conversations', 'dataset-promotion', windowSeconds],
    queryFn: async () => {
      const response = await listConversations({
        query: { window_seconds: windowParam(windowSeconds), limit: 100 },
      });
      if (!response.data) throw new Error('Could not load sessions.');
      return response.data.conversations;
    },
  });
  const agentRows = useQuery({
    queryKey: ['dimensions', 'agent', 'dataset-promotion', windowSeconds],
    queryFn: async () => {
      const response = await listDimension({
        path: { kind: 'agent' as DimensionKind },
        query: { window_seconds: windowParam(windowSeconds), limit: 100 },
      });
      if (!response.data) throw new Error('Could not load agents.');
      return response.data.rows;
    },
  });

  React.useEffect(() => {
    if (!session && conversations.data?.[0]) setSession(conversations.data[0].conversation_id);
  }, [session, conversations.data]);
  React.useEffect(() => {
    if (!agent && agentRows.data?.[0]) setAgent(agentRows.data[0].key);
  }, [agent, agentRows.data]);

  const pipeline = React.useMemo(
    () => promotionPipeline(scope, session, agent, agents, windowSeconds, engine),
    [scope, session, agent, agents, windowSeconds, engine],
  );
  const selectionReady =
    (scope === 'session' && !!session) ||
    (scope === 'agent' && !!agent) ||
    (scope === 'agents' && agents.length > 0);

  const simulate = useMutation({
    mutationFn: () => simulateQuery(pipeline, windowParam(windowSeconds)),
  });
  const execute = useMutation({
    mutationFn: async () => {
      const result = await runQuery(pipeline, windowParam(windowSeconds));
      if (result.truncated) {
        throw new Error(
          'More than 1,000 runs matched. Narrow the scope before saving the dataset.',
        );
      }
      const response = await publishDataset({
        body: {
          name: datasetName,
          description: promotionDescription(scope, session, agent, agents),
          pipeline,
          engine,
          columns: result.columns,
          items: result.rows,
          source: result.source,
          window_seconds: result.window_seconds ?? undefined,
        },
      });
      return { result, published: answerOf(response, 'The dataset could not be saved.') };
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['datasets'] }),
  });

  const selectedDataset =
    catalog.data?.find((candidate) => candidate.name === search.dataset) ?? catalog.data?.[0];
  const selectedVersion =
    selectedDataset?.versions.find((candidate) => candidate.version === search.version)?.version ??
    selectedDataset?.latest.version;
  const activeView = search.view ?? (selectedDataset ? 'rows' : 'promote');

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Datasets</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Versioned cases for evaluation. Promote retained production runs by one session, one
            agent, or any of several agents; review the generated query before it writes a version.
            Or discover a public corpus on Kaggle or Hugging Face and import it into an annotation
            project &mdash; where what the mirror claims about a licence and what somebody actually
            read stay two different fields.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <TimeRange
            value={windowSeconds}
            onChange={(window) =>
              void navigate({ search: (previous) => ({ ...previous, window }) })
            }
          />
          <Button
            size="sm"
            variant={activeView === 'promote' ? 'default' : 'outline'}
            onClick={() =>
              void navigate({ search: (previous) => ({ ...previous, view: 'promote' }) })
            }
          >
            Build dataset
          </Button>
          <Button
            size="sm"
            variant={activeView === 'discover' ? 'default' : 'outline'}
            onClick={() =>
              void navigate({ search: (previous) => ({ ...previous, view: 'discover' }) })
            }
          >
            <Globe className="h-3.5 w-3.5" /> Discover
          </Button>
        </div>
      </div>

      <div className="grid gap-4 xl:grid-cols-[minmax(18rem,23rem)_minmax(0,1fr)]">
        <DatasetCatalog
          datasets={catalog.data ?? []}
          pending={catalog.isPending}
          error={catalog.error}
          selected={selectedDataset?.name}
          onSelect={(dataset) =>
            void navigate({
              search: (previous) => ({
                ...previous,
                dataset: dataset.name,
                version: dataset.latest.version,
                view: 'rows',
                q: undefined,
              }),
            })
          }
        />

        {activeView === 'discover' ? (
          <HubDiscovery
            project={search.project ?? ''}
            onProjectChange={(project) =>
              void navigate({ search: (previous) => ({ ...previous, project }), replace: true })
            }
            windowSeconds={windowSeconds}
          />
        ) : activeView === 'promote' ? (
          <div className="flex min-w-0 flex-col gap-4">
            <Card className="overflow-hidden">
              <div className="flex items-start gap-3 border-b border-border p-4">
                <div className="rounded-md bg-primary/10 p-2 text-primary">
                  <Users className="h-4 w-4" />
                </div>
                <div>
                  <h2 className="text-sm font-semibold">Promote conversations</h2>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    Each matching run becomes one source-linked item. Expected output stays open for
                    review and labelling.
                  </p>
                </div>
              </div>

              <div className="flex flex-col gap-4 p-4">
                <div className="flex flex-wrap gap-1">
                  {(
                    [
                      ['session', 'One session'],
                      ['agent', 'One agent'],
                      ['agents', 'Many agents'],
                    ] as const
                  ).map(([value, label]) => (
                    <Button
                      key={value}
                      size="sm"
                      variant={scope === value ? 'default' : 'outline'}
                      onClick={() => setScope(value)}
                    >
                      {label}
                    </Button>
                  ))}
                </div>

                {scope === 'session' ? (
                  <label className="flex flex-col gap-1 text-xs">
                    <span className="text-muted-foreground">Session</span>
                    <select
                      value={session}
                      onChange={(event) => setSession(event.target.value)}
                      className={CONTROL}
                    >
                      <option value="">Choose a session</option>
                      {conversations.data?.map((row) => (
                        <option key={row.conversation_id} value={row.conversation_id}>
                          {row.conversation_id} · {row.runs} runs ·{' '}
                          {row.agents.join(', ') || 'no agent'}
                        </option>
                      ))}
                    </select>
                  </label>
                ) : null}

                {scope === 'agent' ? (
                  <label className="flex flex-col gap-1 text-xs">
                    <span className="text-muted-foreground">Agent</span>
                    <select
                      value={agent}
                      onChange={(event) => setAgent(event.target.value)}
                      className={CONTROL}
                    >
                      <option value="">Choose an agent</option>
                      {agentRows.data?.map((row) => (
                        <option key={row.key} value={row.key}>
                          {row.key} · {row.runs} runs
                        </option>
                      ))}
                    </select>
                  </label>
                ) : null}

                {scope === 'agents' ? (
                  <div className="flex flex-col gap-1 text-xs">
                    <span className="text-muted-foreground">
                      Include runs involving any selected agent
                    </span>
                    <div className="grid max-h-40 gap-1 overflow-y-auto rounded-md border border-border p-2 sm:grid-cols-2">
                      {agentRows.data?.map((row) => (
                        <label
                          key={row.key}
                          className="flex items-center gap-2 rounded px-2 py-1 hover:bg-accent/40"
                        >
                          <input
                            type="checkbox"
                            checked={agents.includes(row.key)}
                            onChange={() =>
                              setAgents((current) =>
                                current.includes(row.key)
                                  ? current.filter((value) => value !== row.key)
                                  : [...current, row.key],
                              )
                            }
                          />
                          <span className="truncate text-sm">{row.key}</span>
                          <span className="ml-auto text-[10px] text-muted-foreground">
                            {row.runs}
                          </span>
                        </label>
                      ))}
                    </div>
                  </div>
                ) : null}

                <label className="flex flex-col gap-1 text-xs">
                  <span className="text-muted-foreground">
                    Dataset name · slashes create folders
                  </span>
                  <input
                    value={datasetName}
                    onChange={(event) => setDatasetName(event.target.value)}
                    className={CONTROL}
                  />
                </label>

                <div>
                  <div className="mb-1 flex items-center justify-between gap-2">
                    <span className="text-xs text-muted-foreground">
                      Generated {ENGINE_LABEL[engine]}
                    </span>
                    <Link
                      to="/data-curation"
                      search={{
                        q: pipeline,
                        name: `promotion/${scope}`,
                        dataset: datasetName,
                        window: search.window,
                      }}
                      className="flex items-center gap-1 text-xs text-primary hover:underline"
                    >
                      Edit in Data Curation <ExternalLink className="h-3 w-3" />
                    </Link>
                  </div>
                  <pre className="id max-h-72 overflow-auto rounded-md border border-border bg-muted/30 p-3 text-xs">
                    {pipeline}
                  </pre>
                </div>

                {available.data === false ? (
                  <EmptyState
                    title="The query engine is not running"
                    hint="Start it with `just query-serve` to simulate or execute."
                  />
                ) : null}

                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    onClick={() => simulate.mutate()}
                    disabled={
                      available.data !== true ||
                      !selectionReady ||
                      simulate.isPending ||
                      execute.isPending
                    }
                  >
                    {simulate.isPending ? <Spinner /> : <Sparkles className="h-3.5 w-3.5" />}{' '}
                    Simulate
                  </Button>
                  <Button
                    onClick={() => execute.mutate()}
                    disabled={
                      available.data !== true ||
                      !selectionReady ||
                      !datasetName.trim() ||
                      simulate.isPending ||
                      execute.isPending
                    }
                  >
                    {execute.isPending ? <Spinner /> : <Play className="h-3.5 w-3.5" />} Build
                    dataset
                  </Button>
                </div>
              </div>
            </Card>

            {execute.data ? (
              <Card className="border-success/40 p-3 text-sm">
                Saved <strong>{execute.data.published.dataset.name}</strong>@
                <code className="id">
                  {execute.data.published.dataset.latest.version.slice(0, 12)}
                </code>{' '}
                · {execute.data.published.dataset.latest.row_count} items.
              </Card>
            ) : null}
            <FlowResultView
              result={execute.data?.result ?? simulate.data}
              error={execute.error ?? simulate.error}
              emptyTitle="No promotion simulated yet"
            />
            <Card className="p-3 text-xs leading-relaxed text-muted-foreground">
              aiwatcher intentionally strips prompt and completion bodies from retained spans.
              Promotion keeps source run, session and trace identifiers plus the selected metadata;
              add an expected output during review, or use an events pipeline when the producer
              deliberately recorded a bounded input field.
            </Card>
          </div>
        ) : selectedDataset && selectedVersion ? (
          <DatasetExplorer
            dataset={selectedDataset}
            versionId={selectedVersion}
            view={activeView as DatasetView}
            search={search.q}
            onVersionChange={(version) =>
              void navigate({
                search: (previous) => ({ ...previous, version, q: undefined }),
              })
            }
            onViewChange={(view) =>
              void navigate({ search: (previous) => ({ ...previous, view }) })
            }
            onSearchChange={(q) =>
              void navigate({ search: (previous) => ({ ...previous, q }), replace: true })
            }
          />
        ) : (
          <Card className="p-4">
            <EmptyState
              title="No collection selected"
              hint="Build a dataset from retained conversations to open its rows here."
            />
          </Card>
        )}
      </div>
    </div>
  );
}

const CONTROL =
  'h-9 rounded-md border border-border bg-card px-3 text-card-foreground text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary';

function DatasetCatalog({
  datasets,
  pending,
  error,
  selected,
  onSelect,
}: {
  datasets: DatasetSummary[];
  pending: boolean;
  error: Error | null;
  selected?: string;
  onSelect: (dataset: DatasetSummary) => void;
}) {
  return (
    <Card className="h-fit overflow-hidden">
      <div className="flex items-center gap-2 border-b border-border p-4">
        <Database className="h-4 w-4 text-primary" />
        <div>
          <h2 className="text-sm font-semibold">Collections</h2>
          <p className="text-xs text-muted-foreground">Immutable versions, latest first.</p>
        </div>
      </div>
      {pending ? (
        <p className="p-4 text-sm text-muted-foreground">Loading datasets…</p>
      ) : error ? (
        <p className="p-4 text-sm text-danger">{error.message}</p>
      ) : datasets.length === 0 ? (
        <div className="p-4">
          <EmptyState
            title="No datasets yet"
            hint="Simulate a promotion, then build its first version."
          />
        </div>
      ) : (
        <div className="divide-y divide-border/50">
          {datasets.map((dataset) => (
            <button
              type="button"
              key={dataset.name}
              onClick={() => onSelect(dataset)}
              className={cn(
                'w-full p-4 text-left transition-colors hover:bg-accent/30',
                selected === dataset.name && 'bg-accent/50',
              )}
            >
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{dataset.name}</p>
                  {dataset.description ? (
                    <p className="mt-0.5 text-xs text-muted-foreground">{dataset.description}</p>
                  ) : null}
                </div>
                <Badge>
                  {dataset.versions.length} version{dataset.versions.length === 1 ? '' : 's'}
                </Badge>
              </div>
              <div className="mt-2 flex flex-wrap gap-x-2 text-[11px] text-muted-foreground">
                <span>{dataset.latest.row_count} items</span>
                <span>·</span>
                <code className="id">{dataset.latest.version.slice(0, 12)}</code>
                <span>·</span>
                <span>{new Date(dataset.latest.created_at).toLocaleString()}</span>
              </div>
            </button>
          ))}
        </div>
      )}
    </Card>
  );
}

function promotionPipeline(
  scope: PromotionScope,
  session: string,
  agent: string,
  agents: string[],
  windowSeconds: number,
  engine: QueryEngineName,
): string {
  switch (engine) {
    case 'flow':
      return promotionFlow(scope, session, agent, agents, windowSeconds);
    case 'datafusion':
      return promotionDataFusion(scope, session, agent, agents, windowSeconds);
    case 'duckdb':
      return promotionDuckDB(scope, session, agent, agents, windowSeconds);
  }
}

/**
 * The same promotion in DataFusion's Python API (AW-3): the runs, narrowed to a
 * session or to the agents chosen — one row per agent first, as Flow's
 * `array_expand` gives — then one row per run, renamed to the dataset contract.
 * Every value goes through `JSON.stringify`, whose string is a Python literal.
 */
function promotionDataFusion(
  scope: PromotionScope,
  session: string,
  agent: string,
  agents: string[],
  windowSeconds: number,
): string {
  const lines = ['(', `    read("default", period=${pythonPeriod(windowSeconds)})`];
  if (scope === 'session') {
    lines.push(`    .filter(col("conversation_id") == lit(${JSON.stringify(session)}))`);
  } else {
    const selected = scope === 'agent' ? [agent] : agents;
    const terms = (selected.length > 0 ? selected : ['__choose_an_agent__']).map(
      (value) => `col("selected_agent") == lit(${JSON.stringify(value)})`,
    );
    lines.push(
      '    .with_column("selected_agent", col("agents"))',
      '    .unnest_columns("selected_agent")',
      `    .filter(${terms.length > 1 ? terms.map((term) => `(${term})`).join(' | ') : terms[0]})`,
    );
  }
  lines.push(
    '    .distinct_on(',
    '        [col("run_id")],',
    '        [',
    '            col("run_id").alias("source_run_id"),',
    '            col("conversation_id").alias("source_session_id"),',
    '            col("trace_id").alias("source_trace_id"),',
    '            col("agents"),',
    '            col("status"),',
    '            col("started_at"),',
    '        ],',
    '        [col("run_id").sort()],',
    '    )',
    ')',
  );
  return lines.join('\n');
}

/**
 * The same promotion in DuckDB's relational API (AW-3): one row per agent
 * through `unnest`, then one row per run as an aggregate over it — a relation
 * has no `distinct_on`, and a column that does not vary within a run is read
 * with `any_value`. Every value goes through `JSON.stringify`, and the group
 * key is a bare column name, the one text a strict DuckDB query hands a relation.
 */
function promotionDuckDB(
  scope: PromotionScope,
  session: string,
  agent: string,
  agents: string[],
  windowSeconds: number,
): string {
  const constant = (value: string) => `ConstantExpression(${JSON.stringify(value)})`;
  const lines = ['(', `    read("default", period=${pythonPeriod(windowSeconds)})`];
  if (scope === 'session') {
    lines.push(`    .filter(ColumnExpression("conversation_id") == ${constant(session)})`);
  } else {
    const selected = scope === 'agent' ? [agent] : agents;
    const terms = (selected.length > 0 ? selected : ['__choose_an_agent__']).map(
      (value) => `ColumnExpression("selected_agent") == ${constant(value)}`,
    );
    lines.push(
      '    .project(StarExpression(), FunctionExpression("unnest", ColumnExpression("agents")).alias("selected_agent"))',
      `    .filter(${terms.length > 1 ? terms.map((term) => `(${term})`).join(' | ') : terms[0]})`,
    );
  }
  const kept = (column: string, alias = column) =>
    `            FunctionExpression("any_value", ColumnExpression("${column}")).alias("${alias}"),`;
  lines.push(
    '    .aggregate(',
    '        [',
    '            ColumnExpression("run_id").alias("source_run_id"),',
    kept('conversation_id', 'source_session_id'),
    kept('trace_id', 'source_trace_id'),
    kept('agents'),
    kept('status'),
    kept('started_at'),
    '        ],',
    '        "run_id",',
    '    )',
    '    .sort(ColumnExpression("source_run_id").asc())',
    ')',
  );
  return lines.join('\n');
}

function promotionFlow(
  scope: PromotionScope,
  session: string,
  agent: string,
  agents: string[],
  windowSeconds: number,
): string {
  const lines = ['data_frame()', `    ->read(default, period: ${flowPeriod(windowSeconds)})`];
  if (scope === 'session') {
    lines.push(`    ->filter(ref('conversation_id')->same(lit('${phpString(session)}')))`);
  } else {
    lines.push("    ->withEntry('selected_agent', array_expand(ref('agents')))");
    const selected = scope === 'agent' ? [agent] : agents;
    const conditions = selected.map(
      (value) => `ref('selected_agent')->same(lit('${phpString(value)}'))`,
    );
    lines.push(
      conditions.length > 1
        ? `    ->filter(any(\n        ${conditions.join(',\n        ')}\n    ))`
        : `    ->filter(${conditions[0] ?? "ref('selected_agent')->same(lit('__choose_an_agent__'))"})`,
    );
    lines.push("    ->dropDuplicates(ref('run_id'))");
  }
  lines.push(
    "    ->rename('run_id', 'source_run_id')",
    "    ->rename('conversation_id', 'source_session_id')",
    "    ->rename('trace_id', 'source_trace_id')",
    '    ->select(',
    "        ref('source_run_id'),",
    "        ref('source_session_id'),",
    "        ref('source_trace_id'),",
    "        ref('agents'),",
    "        ref('status'),",
    "        ref('started_at')",
    '    )',
    '    ->write(to_output(truncate: false))',
    '    ->run();',
  );
  return lines.join('\n');
}

function flowPeriod(seconds: number): string {
  const presets = new Map<number, string>([
    [900, '15m'],
    [3_600, '1h'],
    [21_600, '6h'],
    [86_400, '24h'],
    [604_800, '7d'],
  ]);
  if (seconds === 0) return "'all'";
  const preset = presets.get(seconds);
  return preset ? `'${preset}'` : String(seconds);
}

/** Flow's period as Python writes it: a word double-quoted, seconds bare. */
function pythonPeriod(seconds: number): string {
  const period = flowPeriod(seconds);
  return period.startsWith("'") ? JSON.stringify(period.slice(1, -1)) : period;
}

function promotionDescription(
  scope: PromotionScope,
  session: string,
  agent: string,
  agents: string[],
): string {
  if (scope === 'session') return `Production runs promoted from session ${session}.`;
  if (scope === 'agent') return `Production runs involving agent ${agent}.`;
  return `Production runs involving any of: ${agents.join(', ')}.`;
}

function phpString(value: string): string {
  return value.replaceAll('\\', '\\\\').replaceAll("'", "\\'");
}
