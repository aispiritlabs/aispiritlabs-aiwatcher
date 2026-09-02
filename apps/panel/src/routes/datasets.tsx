import * as React from 'react';
import { createFileRoute, Link } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Database, ExternalLink, Globe, Play, Sparkles, Users } from 'lucide-react';
import { z } from 'zod';

import {
  listConversations,
  listDatasets,
  listDimension,
  publishDataset,
} from '@/api/generated/sdk.gen';
import type { DatasetSummary, DimensionKind } from '@/api/generated/types.gen';
import { DatasetExplorer, type DatasetView } from '@/components/dataset-explorer';
import { FlowResultView } from '@/components/flow-preview';
import { HubDiscovery } from '@/components/hub-discovery';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import { isFlowAvailable, runQuery, simulateQuery } from '@/lib/flow';
import { cn } from '@/lib/utils';

const searchSchema = z.object({
  window: z.number().int().nonnegative().optional(),
  dataset: z.string().optional(),
  version: z.string().optional(),
  view: z.enum(['rows', 'evaluations', 'lineage', 'promote', 'discover']).optional(),
  q: z.string().optional(),
  /** Which annotation project a hub import lands in. In the URL so a link
   *  to a half-configured import is a link somebody else can finish. */
  project: z.string().optional(),
});

export const Route = createFileRoute('/datasets')({
  validateSearch: searchSchema,
  component: DatasetsPage,
});

type PromotionScope = 'session' | 'agent' | 'agents';

function DatasetsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const [scope, setScope] = React.useState<PromotionScope>('session');
  const [session, setSession] = React.useState('');
  const [agent, setAgent] = React.useState('');
  const [agents, setAgents] = React.useState<string[]>([]);
  const [datasetName, setDatasetName] = React.useState('evaluation/promoted-conversations');

  const available = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isFlowAvailable,
    refetchInterval: 10_000,
  });
  const catalog = useQuery({
    queryKey: ['datasets'],
    queryFn: async () => {
      const response = await listDatasets();
      if (!response.data) throw apiError(response.error, 'Could not load datasets.');
      return response.data.datasets;
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
    () => promotionPipeline(scope, session, agent, agents, windowSeconds),
    [scope, session, agent, agents, windowSeconds],
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
          columns: result.columns,
          items: result.rows,
          source: result.source,
          window_seconds: result.window_seconds ?? undefined,
        },
      });
      if (!response.data) throw apiError(response.error, 'The dataset could not be saved.');
      return { result, published: response.data };
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
            agent, or any of several agents; review the generated Flow PHP before it writes a
            version. Or discover a public corpus on Kaggle or Hugging Face and import it into an
            annotation project &mdash; where what the mirror claims about a licence and what
            somebody actually read stay two different fields.
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
                    <span className="text-xs text-muted-foreground">Generated Flow PHP</span>
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
                    title="Flow is not running"
                    hint="Start it with `just flow-serve` to simulate or execute."
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
