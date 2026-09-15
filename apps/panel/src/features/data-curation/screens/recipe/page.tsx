import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getRouteApi } from '@tanstack/react-router';
import { Beaker, Play, Save, Sparkles } from 'lucide-react';
import * as React from 'react';

import { listRecipes, publishDataset, publishDatasetSample, saveRecipe } from '@/api/generated/sdk.gen';
import type { CurationRecipe } from '@/api/generated/types.gen';
import { SamplePublication } from '@/shared/components/sample-publication';
import { FlowDiagnostics, FlowResultView } from '@/shared/components/flow-preview';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/shared/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import { contentFor } from '@/shared/lib/engine-content';
import type { QueryExample } from '@/shared/lib/flow';
import {
  ENGINE_LABEL,
  checkQuery,
  isQueryEngineAvailable,
  linkedEngine,
  runQuery,
  simulateQuery,
  useQueryEngine,
  writtenElsewhere,
  type QueryEngineName,
} from '@/shared/lib/query';
import { answerOf } from '@/shared/lib/result';
import { useUnsavedChanges } from '@/shared/lib/unsaved-changes';

const routeApi = getRouteApi('/data-curation/recipe');
const contextOf = (search: { q?: string; writtenFor?: string; name?: string; dataset?: string; description?: string }) =>
  JSON.stringify([search.q, search.writtenFor, search.name, search.dataset, search.description]);

export function DataCurationPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const queryClient = useQueryClient();

  // The engine this deployment runs, and the language its content is in (AW-3).
  const deployed = useQueryEngine().data?.engine ?? undefined;
  const content = contentFor(deployed);
  const language = ENGINE_LABEL[deployed ?? 'flow'];

  // The draft, and the engine it was written for. `null` text is the deployed
  // engine's starter and `null` engine is whichever is deployed, so a page
  // opened before healthz answers still lands on text that runs. A saved recipe
  // carries its own engine, a link carries `writtenFor` (Flow's when it does
  // not), and one written for another is shown, not run.
  const [text, setDraft] = React.useState<string | null>(search.q ?? null);
  const [writtenFor, setWrittenFor] = React.useState<QueryEngineName | null>(
    search.q === undefined ? null : linkedEngine(search.q, search.writtenFor, undefined),
  );
  const draft = text ?? content?.starterCuration ?? '';
  const draftEngine = writtenFor ?? deployed ?? 'flow';
  const foreign = writtenElsewhere(draftEngine, deployed);
  const [name, setName] = React.useState(search.name ?? 'production/successful-sessions');
  const [dataset, setDataset] = React.useState(search.dataset ?? 'evaluation/production-sessions');
  const [description, setDescription] = React.useState(
    search.description ?? 'Cases curated from retained production runs.',
  );
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const snapshot = JSON.stringify({ text, writtenFor, name, dataset, description });
  const [savedSnapshot, setSavedSnapshot] = React.useState(snapshot);
  const currentSnapshot = React.useRef(snapshot);
  currentSnapshot.current = snapshot;
  const mounted = React.useRef(true);
  React.useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const dirty = snapshot !== savedSnapshot;
  const resultSignature = JSON.stringify([draft, draftEngine, windowSeconds, name, dataset, description]);
  const context = contextOf(search);
  const loadedContext = React.useRef(context);
  const generation = React.useRef(0);

  const available = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isQueryEngineAvailable,
    refetchInterval: 10_000,
  });
  const recipes = useQuery({
    queryKey: ['curations'],
    queryFn: async () => {
      const response = await listRecipes();
      return answerOf(response, 'Could not load saved recipes.').recipes;
    },
  });

  const test = useMutation({ mutationFn: () => checkQuery(draft) });
  const simulate = useMutation({
    mutationFn: async () => {
      const result = await simulateQuery(draft, windowParam(windowSeconds));
      return { ...result, signature: resultSignature,
        sample: { mode: 'preview' as const, truncated_stages: result.truncated ? ['query'] : [] } };
    },
  });
  const save = useMutation({
    mutationFn: async () => {
      const contextGeneration = generation.current;
      const checked = await checkQuery(draft);
      if (!checked.ok)
        throw new Error(checked.diagnostics[0]?.message ?? 'The pipeline is invalid.');
      const response = await saveRecipe({
        body: { name, description, pipeline: draft, engine: draftEngine },
      });
      return { ...answerOf(response, 'Could not save the recipe.'), snapshot, contextGeneration,
        search: { q: draft, writtenFor: draftEngine, name, dataset, description } };
    },
    onSuccess: (stored) => {
      void queryClient.invalidateQueries({ queryKey: ['curations'] });
      if (!mounted.current || generation.current !== stored.contextGeneration) return;
      setSavedSnapshot(stored.snapshot);
      if (currentSnapshot.current === stored.snapshot) {
        loadedContext.current = contextOf(stored.search);
        void navigate({ search: (previous) => ({ ...previous, ...stored.search }),
          replace: true, ignoreBlocker: true });
      }
    },
  });
  const execute = useMutation({
    mutationFn: async () => {
      const responseResult = await runQuery(draft, windowParam(windowSeconds));
      const result = { ...responseResult, signature: resultSignature,
        sample: responseResult.truncated ? { mode: 'truncated' as const, truncated_stages: ['query'] } : undefined };
      if (result.truncated) return { result, published: undefined };
      const response = await publishDataset({
        body: {
          name: dataset,
          description,
          recipe: name || undefined,
          pipeline: draft,
          engine: draftEngine,
          columns: result.columns,
          items: result.rows,
          source: result.source,
          window_seconds: result.window_seconds ?? undefined,
        },
      });
      return {
        result,
        published: answerOf(response, 'The pipeline ran, but its dataset could not be saved.'),
      };
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['datasets'] }),
  });

  const latestResult = execute.submittedAt > simulate.submittedAt ? execute.data?.result : simulate.data;
  const publishSample = useMutation({
    mutationFn: async () => {
      if (!latestResult?.sample || latestResult.signature !== resultSignature) {
        throw new Error('Run again before publishing a sample of the current recipe.');
      }
      const response = await publishDatasetSample({ body: {
        name: `${dataset.trim()}/samples`, description, recipe: name || undefined,
        pipeline: draft, engine: draftEngine, columns: latestResult.columns, items: latestResult.rows,
        source: latestResult.source, window_seconds: latestResult.window_seconds ?? undefined,
        sample: latestResult.sample,
      } });
      return answerOf(response, 'Could not publish the sample.');
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['datasets'] }),
  });

  React.useEffect(() => { publishSample.reset(); }, [simulate.submittedAt, execute.submittedAt]);

  // An example is a recipe nobody saved, so it fills in the two boxes a saved
  // one already carries — and lands in the URL like every other filter here,
  // which is what makes "look at this curation" a link rather than a paragraph.
  const loadExample = (example: QueryExample) => {
    if (!confirmDiscard()) return;
    generation.current += 1;
    loadedContext.current = contextOf({ q: example.query, writtenFor: deployed ?? 'flow', name: example.name, dataset: example.dataset, description: example.description });
    setName(example.name);
    setDataset(example.dataset);
    setDescription(example.description);
    setDraft(example.query);
    setWrittenFor(null);
    test.reset();
    simulate.reset();
    execute.reset();
    save.reset();
    publishSample.reset();
    void navigate({
      search: (previous) => ({
        ...previous,
        q: example.query,
        writtenFor: deployed ?? 'flow',
        name: example.name,
        dataset: example.dataset,
        description: example.description,
      }),
      ignoreBlocker: true,
    });
  };

  const loadRecipe = (recipe: CurationRecipe) => {
    if (!confirmDiscard()) return;
    generation.current += 1;
    const next = { q: recipe.pipeline, writtenFor: recipe.engine ?? 'flow',
      name: recipe.name, dataset, description: recipe.description ?? '' };
    loadedContext.current = contextOf(next);
    void navigate({ search: (previous) => ({ ...previous, ...next }), ignoreBlocker: true });
    setName(recipe.name);
    setDescription(recipe.description ?? '');
    setDraft(recipe.pipeline);
    setWrittenFor(recipe.engine ?? 'flow');
    setSavedSnapshot(JSON.stringify({ text: recipe.pipeline, writtenFor: recipe.engine ?? 'flow',
      name: recipe.name, dataset, description: recipe.description ?? '' }));
    test.reset();
    simulate.reset();
    execute.reset();
    save.reset();
    publishSample.reset();
  };

  const flowReady = available.data === true && !foreign;
  const busy = test.isPending || simulate.isPending || execute.isPending || save.isPending || publishSample.isPending;
  const confirmDiscard = useUnsavedChanges({ dirty, pending: busy,
    message: 'This recipe has unsaved changes.',
    losesDraft: ({ next }) => next.routeId !== '/data-curation/recipe' || contextOf(next.search) !== context,
  });

  React.useEffect(() => {
    if (loadedContext.current === context) return;
    loadedContext.current = context;
    generation.current += 1;
    const restored = {
      text: search.q ?? null,
      writtenFor: search.q === undefined ? null : linkedEngine(search.q, search.writtenFor, undefined),
      name: search.name ?? 'production/successful-sessions',
      dataset: search.dataset ?? 'evaluation/production-sessions',
      description: search.description ?? 'Cases curated from retained production runs.',
    };
    setDraft(restored.text);
    setWrittenFor(restored.writtenFor);
    setName(restored.name);
    setDataset(restored.dataset);
    setDescription(restored.description);
    setSavedSnapshot(JSON.stringify(restored));
    test.reset(); simulate.reset(); execute.reset(); save.reset(); publishSample.reset();
  }, [context]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">One script</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Turn retained production data into reproducible, versioned datasets. Write the
            transformation here in {language}: pin its period in read(), test without reading rows,
            simulate 25 cases, then execute and save the exact output.
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(window) => void navigate({ search: (previous) => ({ ...previous, window }) })}
        />
      </div>

      {dirty ? <p role="status" className="text-xs text-warning">Unsaved recipe changes.</p> : null}

      {available.data === false ? (
        <EmptyState
          title="The query engine is not running"
          hint="Start it with `just query-serve`. Saved recipes and datasets remain readable from the Rust registry."
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
            {foreign ? (
              <p className="border-b border-border bg-warning/10 px-3 py-2 text-xs text-warning">
                {foreign}
              </p>
            ) : null}
            <textarea
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              readOnly={Boolean(foreign)}
              aria-label={`${ENGINE_LABEL[draftEngine]} recipe`}
              spellCheck={false}
              rows={17}
              className="id w-full resize-y bg-transparent p-3 outline-none"
            />
            <div className="flex flex-wrap items-center gap-2 border-t border-border p-3">
              <Button variant="outline" onClick={() => test.mutate()} disabled={!flowReady || busy}>
                {test.isPending ? <Spinner /> : <Beaker className="h-3.5 w-3.5" />} Test
              </Button>
              <Button
                variant="outline"
                onClick={() => simulate.mutate()}
                disabled={!flowReady || busy}
              >
                {simulate.isPending ? <Spinner /> : <Sparkles className="h-3.5 w-3.5" />} Simulate
                25 rows
              </Button>
              <Button
                onClick={() => execute.mutate()}
                disabled={!flowReady || busy || !dataset.trim()}
              >
                {execute.isPending ? <Spinner /> : <Play className="h-3.5 w-3.5" />} Execute &amp;
                save dataset
              </Button>
              <Button
                variant="ghost"
                onClick={() => save.mutate()}
                disabled={busy || !name.trim() || Boolean(foreign)}
              >
                {save.isPending ? <Spinner /> : <Save className="h-3.5 w-3.5" />} Save script
              </Button>
            </div>
          </Card>

          <FlowDiagnostics check={test.data} />
          {test.error ? <ErrorCard error={test.error} /> : null}
          {save.data ? (
            <Card className="border-success/40 p-3 text-sm">
              Recipe <strong>{save.data.recipe.name}</strong>{' '}
              {save.data.created ? 'saved as a new revision' : 'was already saved'}.
            </Card>
          ) : null}
          {save.error ? <ErrorCard error={save.error} /> : null}
          {execute.data?.published ? (
            <Card className="border-success/40 p-3 text-sm">
              Dataset <strong>{execute.data.published.dataset.name}</strong> saved as version{' '}
              <code className="id">
                {execute.data.published.dataset.latest.version.slice(0, 12)}
              </code>{' '}
              with {execute.data.published.dataset.latest.row_count} rows.
            </Card>
          ) : null}
          {latestResult?.sample ? <SamplePublication
            name={`${dataset.trim()}/samples`} rowCount={latestResult.rows.length} sample={latestResult.sample}
            pending={publishSample.isPending}
            disabled={busy || !dataset.trim() || latestResult.signature !== resultSignature}
            error={publishSample.error} published={publishSample.data}
            onPublish={() => publishSample.mutate()}
          /> : null}
          <FlowResultView
            result={latestResult}
            error={execute.error ?? simulate.error}
          />
        </div>

        <div className="flex flex-col gap-4">
          <Card className="overflow-hidden">
            <div className="border-b border-border p-3 text-xs font-semibold">Examples</div>
            {content ? null : (
              <p className="p-3 text-xs text-muted-foreground">
                No examples ship for {language} in this build yet.
              </p>
            )}
            <div className="divide-y divide-border/50">
              {(content?.examples ?? []).map((example) => (
                <button
                  key={example.name}
                  type="button"
                  onClick={() => loadExample(example)}
                  disabled={busy}
                  className="w-full p-3 text-left hover:bg-accent/40"
                >
                  <p className="truncate text-sm font-medium">{example.title}</p>
                  <p className="mt-0.5 text-[11px] text-muted-foreground">{example.description}</p>
                </button>
              ))}
            </div>
          </Card>

          <Card className="overflow-hidden">
            <div className="border-b border-border p-3 text-xs font-semibold">Transformations</div>
            <div className="divide-y divide-border/50">
              {(content?.transformations ?? []).map(([label, example, help]) => (
                <div key={label} className="p-3">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-medium">{label}</span>
                    <Badge>{language}</Badge>
                  </div>
                  <code className="id mt-1 block overflow-x-auto text-[11px] text-primary">
                    {example}
                  </code>
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
                    disabled={busy}
                    className="w-full p-3 text-left hover:bg-accent/40"
                  >
                    <p className="truncate text-sm font-medium">{recipe.name}</p>
                    <p className="mt-0.5 text-[11px] text-muted-foreground">
                      {recipe.revision.slice(0, 10)} · {new Date(recipe.saved_at).toLocaleString()}
                      {recipe.engine && recipe.engine !== 'flow'
                        ? ` · ${ENGINE_LABEL[recipe.engine]}`
                        : ''}
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

function ErrorCard({ error }: { error: Error }) {
  return <Card className="border-danger/40 p-3 text-sm text-danger">{error.message}</Card>;
}
