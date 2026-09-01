import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Beaker, Play, Save, Sparkles } from 'lucide-react';
import { z } from 'zod';

import { listRecipes, publishDataset, saveRecipe } from '@/api/generated/sdk.gen';
import type { CurationRecipe } from '@/api/generated/types.gen';
import { EngineLauncher } from '@/components/engine-launcher';
import { FlowDiagnostics, FlowResultView } from '@/components/flow-preview';
import {
  DEFAULT_WINDOW_SECONDS,
  TimeRange,
  windowParam,
} from '@/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import {
  STARTER_CURATION,
  checkQuery,
  isFlowAvailable,
  runQuery,
  simulateQuery,
} from '@/lib/flow';

const searchSchema = z.object({
  q: z.string().optional(),
  name: z.string().optional(),
  dataset: z.string().optional(),
  window: z.number().int().nonnegative().optional(),
  // The engine picker's own selection and search, in the URL like every other
  // filter here: a link to "this launch plan, over this window" is what
  // somebody sends a colleague when a curation needs re-running.
  engine: z.string().optional(),
  engineFind: z.string().optional(),
});

export const Route = createFileRoute('/data-curation')({
  validateSearch: searchSchema,
  component: DataCurationPage,
});

const TRANSFORMATIONS = [
  ['Period', "->read(default, period: '24h')", 'Pin a relative period in the saved recipe.'],
  ['Filter', "->filter(ref('status')->same(lit('succeeded')))", 'Keep cases matching a condition.'],
  ['Enrich', "->withEntry('label', lit('production'))", 'Add labels or derived fields.'],
  ['Expand', "->withEntry('agent', array_expand(ref('agents')))", 'Turn a list into one row per value.'],
  ['Deduplicate', "->dropDuplicates(ref('conversation_id'))", 'Choose the identity of one case.'],
  ['Rename', "->rename('run_id', 'source_run_id')", 'Shape the dataset contract.'],
  ['Combine', '->filter(any(condition_a, condition_b))', 'Match one of several agents or rules.'],
  ['Aggregate', '->groupBy(…)->aggregate(…)', 'Build summaries or grouped cases.'],
] as const;

function DataCurationPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const [draft, setDraft] = React.useState(search.q ?? STARTER_CURATION);
  const [name, setName] = React.useState(search.name ?? 'production/successful-sessions');
  const [dataset, setDataset] = React.useState(search.dataset ?? 'evaluation/production-sessions');
  const [description, setDescription] = React.useState('Cases curated from retained production runs.');
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const available = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isFlowAvailable,
    refetchInterval: 10_000,
  });
  const recipes = useQuery({
    queryKey: ['curations'],
    queryFn: async () => {
      const response = await listRecipes();
      if (!response.data) throw apiError(response.error, 'Could not load saved recipes.');
      return response.data.recipes;
    },
  });

  const test = useMutation({ mutationFn: () => checkQuery(draft) });
  const simulate = useMutation({
    mutationFn: () => simulateQuery(draft, windowParam(windowSeconds)),
  });
  const save = useMutation({
    mutationFn: async () => {
      const checked = await checkQuery(draft);
      if (!checked.ok) throw new Error(checked.diagnostics[0]?.message ?? 'The pipeline is invalid.');
      const response = await saveRecipe({ body: { name, description, pipeline: draft } });
      if (!response.data) throw apiError(response.error, 'Could not save the recipe.');
      return response.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['curations'] });
      void navigate({ search: { q: draft, name, dataset, window: search.window }, replace: true });
    },
  });
  const execute = useMutation({
    mutationFn: async () => {
      const result = await runQuery(draft, windowParam(windowSeconds));
      if (result.truncated) {
        throw new Error(
          'The result exceeds the 1,000-row execution cap. Narrow the filters or add an explicit limit before saving a version.',
        );
      }
      const response = await publishDataset({
        body: {
          name: dataset,
          description,
          recipe: name || undefined,
          pipeline: draft,
          columns: result.columns,
          items: result.rows,
          source: result.source,
          window_seconds: result.window_seconds ?? undefined,
        },
      });
      if (!response.data) throw apiError(response.error, 'The pipeline ran, but its dataset could not be saved.');
      return { result, published: response.data };
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['datasets'] }),
  });

  const loadRecipe = (recipe: CurationRecipe) => {
    setName(recipe.name);
    setDescription(recipe.description ?? '');
    setDraft(recipe.pipeline);
    test.reset();
    simulate.reset();
    execute.reset();
  };

  const flowReady = available.data === true;
  const busy = test.isPending || simulate.isPending || execute.isPending || save.isPending;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Data Curation</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Turn retained production data into reproducible, versioned datasets. Either start a
            curation workflow the orchestrator already holds and set only what, where and over what
            period, or write the transformation here in Flow PHP: pin its period in read(), test
            without reading rows, simulate 25 cases, then execute and save the exact output.
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(window) => void navigate({ search: (previous) => ({ ...previous, window }) })}
        />
      </div>

      {/* The orchestrated route, first: a workflow somebody registered is
          already able to do this, and the editor below is for when there is
          not one. */}
      <EngineLauncher
        stage="curation"
        title="Run a registered curation workflow"
        summary="What the orchestrator holds, with the inputs it declared. The period above and the dataset name below fill this in; nothing else is sent."
        context={{ dataset, windowSeconds }}
        search={search.engineFind ?? ''}
        onSearchChange={(engineFind) =>
          void navigate({
            search: (previous) => ({ ...previous, engineFind: engineFind || undefined }),
            replace: true,
          })
        }
        selected={search.engine}
        onSelect={(engine) =>
          void navigate({ search: (previous) => ({ ...previous, engine }), replace: true })
        }
      />

      {available.data === false ? (
        <EmptyState
          title="The Flow service is not running"
          hint="Start it with `just flow-serve`. Saved recipes and datasets remain readable from the Rust registry."
        />
      ) : null}

      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_19rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <Card className="overflow-hidden">
            <div className="grid gap-3 border-b border-border p-3 sm:grid-cols-2">
              <Field label="Recipe name">
                <input
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  className="h-9 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
                />
              </Field>
              <Field label="Target dataset">
                <input
                  value={dataset}
                  onChange={(event) => setDataset(event.target.value)}
                  className="h-9 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
                />
              </Field>
              <Field label="Description" className="sm:col-span-2">
                <input
                  value={description}
                  onChange={(event) => setDescription(event.target.value)}
                  className="h-9 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
                />
              </Field>
            </div>
            <textarea
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              spellCheck={false}
              rows={17}
              className="id w-full resize-y bg-transparent p-3 outline-none"
            />
            <div className="flex flex-wrap items-center gap-2 border-t border-border p-3">
              <Button variant="outline" onClick={() => test.mutate()} disabled={!flowReady || busy}>
                {test.isPending ? <Spinner /> : <Beaker className="h-3.5 w-3.5" />} Test
              </Button>
              <Button variant="outline" onClick={() => simulate.mutate()} disabled={!flowReady || busy}>
                {simulate.isPending ? <Spinner /> : <Sparkles className="h-3.5 w-3.5" />} Simulate 25 rows
              </Button>
              <Button onClick={() => execute.mutate()} disabled={!flowReady || busy || !dataset.trim()}>
                {execute.isPending ? <Spinner /> : <Play className="h-3.5 w-3.5" />} Execute &amp; save dataset
              </Button>
              <Button variant="ghost" onClick={() => save.mutate()} disabled={busy || !name.trim()}>
                {save.isPending ? <Spinner /> : <Save className="h-3.5 w-3.5" />} Save script
              </Button>
            </div>
          </Card>

          <FlowDiagnostics check={test.data} />
          {test.error ? <ErrorCard error={test.error} /> : null}
          {save.data ? (
            <Card className="border-success/40 p-3 text-sm">
              Recipe <strong>{save.data.recipe.name}</strong> {save.data.created ? 'saved as a new revision' : 'was already saved'}.
            </Card>
          ) : null}
          {save.error ? <ErrorCard error={save.error} /> : null}
          {execute.data ? (
            <Card className="border-success/40 p-3 text-sm">
              Dataset <strong>{execute.data.published.dataset.name}</strong> saved as version{' '}
              <code className="id">{execute.data.published.dataset.latest.version.slice(0, 12)}</code> with{' '}
              {execute.data.published.dataset.latest.row_count} rows.
            </Card>
          ) : null}
          <FlowResultView
            result={execute.data?.result ?? simulate.data}
            error={execute.error ?? simulate.error}
          />
        </div>

        <div className="flex flex-col gap-4">
          <Card className="overflow-hidden">
            <div className="border-b border-border p-3 text-xs font-semibold">Transformations</div>
            <div className="divide-y divide-border/50">
              {TRANSFORMATIONS.map(([label, example, help]) => (
                <div key={label} className="p-3">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-medium">{label}</span>
                    <Badge>Flow PHP</Badge>
                  </div>
                  <code className="id mt-1 block overflow-x-auto text-[11px] text-primary">{example}</code>
                  <p className="mt-1 text-[11px] text-muted-foreground">{help}</p>
                </div>
              ))}
            </div>
          </Card>

          <Card className="overflow-hidden">
            <div className="border-b border-border p-3 text-xs font-semibold">Saved recipes</div>
            {recipes.isError ? (
              <p className="p-3 text-xs text-danger">{recipes.error.message}</p>
            ) : recipes.data?.length ? (
              <div className="divide-y divide-border/50">
                {recipes.data.map((recipe) => (
                  <button
                    key={`${recipe.name}-${recipe.revision}`}
                    type="button"
                    onClick={() => loadRecipe(recipe)}
                    className="w-full p-3 text-left hover:bg-accent/40"
                  >
                    <p className="truncate text-sm font-medium">{recipe.name}</p>
                    <p className="mt-0.5 text-[11px] text-muted-foreground">
                      {recipe.revision.slice(0, 10)} · {new Date(recipe.saved_at).toLocaleString()}
                    </p>
                  </button>
                ))}
              </div>
            ) : (
              <p className="p-3 text-xs text-muted-foreground">No scripts saved yet.</p>
            )}
          </Card>
        </div>
      </div>
    </div>
  );
}

function Field({
  label,
  className,
  children,
}: {
  label: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <label className={`flex flex-col gap-1 ${className ?? ''}`}>
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function apiError(error: unknown, fallback: string): Error {
  if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') {
    return new Error(error.message);
  }
  return new Error(fallback);
}

function ErrorCard({ error }: { error: Error }) {
  return <Card className="border-danger/40 p-3 text-sm text-danger">{error.message}</Card>;
}
