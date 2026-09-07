import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Play, Plus, Save, ServerCog, Sparkles, Upload } from 'lucide-react';
import { z } from 'zod';

import {
  listPipelines,
  publishDataset,
  savePipeline,
  startExecution,
} from '@/api/generated/sdk.gen';
import type { BlockSpec, CurationPipeline, PipelineBlock } from '@/api/generated/types.gen';
import { BlockInspector } from '@/components/block-inspector';
import { ManagedRunCard, useManagedRun } from '@/components/managed-run';
import { FlowResultView } from '@/components/flow-preview';
import { PipelineCanvas, blockLabel } from '@/components/pipeline-canvas';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import { rejectionDetails } from '@/lib/annotations';
import { isFlowAvailable } from '@/lib/flow';
import { isMlPipelineAvailable } from '@/lib/ml-pipeline';
import {
  PII_DETECTION_EXAMPLE,
  compileFlow,
  orderOf,
  runPipeline,
  withPinnedNotebooks,
  type PipelineOutcomes,
  type PipelineResult,
} from '@/lib/pipeline';

/**
 * A curation, assembled out of blocks and run one engine at a time.
 *
 * Four things happen on this page and they belong to three different systems.
 * The source and the transforms are one Flow PHP query; a notebook block is a
 * marimo notebook the `ml_pipeline` service runs over the rows that query
 * produced; the view publishes an immutable dataset version through the Rust
 * registry. The chain is driven from the browser because that is the only place
 * that can see all three — see ADR_0024.
 *
 * Both notebook-running services are optional and the page says which one is
 * missing rather than failing: a chain of a source and a transform is a
 * perfectly good curation, and it runs with the notebook runtime switched off.
 *
 * There is a second way to run one, and it is the opposite arrangement: **Run
 * on the server** hands the saved revision to `POST /api/v1/executions` and
 * the browser stops being part of it (ADR_0025). The ad-hoc path above stays,
 * because it is what an editor needs — a preview, a block at a time, an answer
 * in the tab you are already looking at. What it is not is a thing to leave
 * running.
 */

const searchSchema = z.object({
  name: z.string().optional(),
  block: z.string().optional(),
  window: z.number().int().nonnegative().optional(),
});

export const Route = createFileRoute('/data-curation/pipeline')({
  validateSearch: searchSchema,
  component: PipelinePage,
});

type Draft = {
  name: string;
  description: string;
  blocks: PipelineBlock[];
  edges: { from: string; to: string }[];
};

const EMPTY_DRAFT: Draft = {
  name: 'curation/untitled',
  description: '',
  blocks: [],
  edges: [],
};

function PipelinePage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const [draft, setDraft] = React.useState<Draft>(EMPTY_DRAFT);
  const [outcomes, setOutcomes] = React.useState<PipelineOutcomes>({});
  const [result, setResult] = React.useState<PipelineResult | null>(null);
  const [problems, setProblems] = React.useState<string[]>([]);

  const flowReady = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isFlowAvailable,
    refetchInterval: 15_000,
  });
  const notebooksReady = useQuery({
    queryKey: ['ml-pipeline', 'available'],
    queryFn: isMlPipelineAvailable,
    refetchInterval: 15_000,
  });
  const saved = useQuery({
    queryKey: ['curation-pipelines'],
    queryFn: async () => {
      const response = await listPipelines();
      if (!response.data) throw new Error('Could not load saved pipelines.');
      return response.data.pipelines;
    },
  });

  const chain = React.useMemo(() => orderOf(draft.blocks, draft.edges), [draft]);
  const selected = draft.blocks.find((block) => block.id === search.block);
  const view = chain?.find((block) => block.spec.kind === 'view');
  const publishTo = view?.spec.kind === 'view' ? view.spec.dataset : undefined;

  const load = (pipeline: CurationPipeline | typeof PII_DETECTION_EXAMPLE) => {
    setDraft({
      name: pipeline.name,
      description: pipeline.description ?? '',
      blocks: pipeline.blocks,
      edges: pipeline.edges,
    });
    setOutcomes({});
    setResult(null);
    setProblems([]);
    void navigate({
      search: (previous) => ({ ...previous, name: pipeline.name, block: undefined }),
    });
  };

  const save = useMutation({
    mutationFn: async () => {
      // The pin is taken here rather than kept in step with every keystroke:
      // what a saved pipeline records is the code that was there when it was
      // saved.
      const blocks = await withPinnedNotebooks(draft.blocks);
      const response = await savePipeline({
        body: {
          name: draft.name,
          description: draft.description,
          blocks,
          edges: draft.edges,
        },
      });
      if (!response.data) throw response.error ?? new Error('The pipeline could not be saved.');
      return response.data;
    },
    onSuccess: (stored) => {
      setProblems([]);
      setDraft((previous) => ({ ...previous, blocks: stored.pipeline.blocks }));
      void queryClient.invalidateQueries({ queryKey: ['curation-pipelines'] });
    },
    // Every reason at once, from the registry, which is the only place that
    // decides whether a chain is runnable.
    onError: (error) => setProblems(rejectionDetails(error)),
  });

  const execute = useMutation({
    mutationFn: async (mode: 'preview' | 'full') => {
      if (!chain) throw new Error('Connect the blocks into one chain first.');
      setOutcomes({});
      return runPipeline({
        chain,
        mode,
        windowSeconds: windowParam(windowSeconds),
        onOutcome: (id, outcome) => setOutcomes((previous) => ({ ...previous, [id]: outcome })),
      });
    },
    onSuccess: (produced) => setResult(produced),
  });

  const publish = useMutation({
    mutationFn: async () => {
      if (!result) throw new Error('Run the pipeline before publishing what it produced.');
      if (!publishTo) throw new Error('The view block has no dataset name.');
      // Saved first, always: the version records the chain that made it, and a
      // chain that was never saved is a reference to nothing. The Flow script
      // alone does not describe this execution — a notebook ran after it.
      const stored = await save.mutateAsync();
      const response = await publishDataset({
        body: {
          name: publishTo,
          description: draft.description,
          pipeline: result.script,
          columns: result.columns,
          items: result.rows as Record<string, never>[],
          source: result.flow.source,
          window_seconds: result.flow.window_seconds ?? undefined,
          produced_by: `${stored.pipeline.name}@${stored.pipeline.revision}`,
        },
      });
      if (!response.data) throw response.error ?? new Error('The dataset could not be saved.');
      return response.data;
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['datasets'] }),
  });

  // The run the server owns, if this page started one. Held by id rather than
  // by object: the projection is re-read from the store on every frame, and a
  // copy here would be the stale one.
  const [executionId, setExecutionId] = React.useState<string>();
  const managed = useManagedRun(executionId);

  const startOnServer = useMutation({
    mutationFn: async () => {
      // Saved first, always. A managed run pins a revision, and a chain that
      // was never saved is a reference to nothing — the same reason Publish
      // saves before it writes a version.
      const stored = await save.mutateAsync();
      const response = await startExecution({
        body: {
          target: { kind: 'curation_pipeline', name: stored.pipeline.name },
          window_seconds: windowParam(windowSeconds) ?? undefined,
        },
      });
      if (!response.data) throw response.error ?? new Error('The run could not be started.');
      return response.data;
    },
    onSuccess: (accepted) => setExecutionId(accepted.execution.execution_id),
    onError: (error) => setProblems(rejectionDetails(error)),
  });

  const busy =
    execute.isPending || save.isPending || publish.isPending || startOnServer.isPending;

  const addBlock = (kind: BlockSpec['kind']) => {
    const id = nextId(kind, draft.blocks);
    const last = draft.blocks[draft.blocks.length - 1];
    const block: PipelineBlock = {
      id,
      title: blockLabel(kind),
      position: { x: (last?.position?.x ?? -300) + 300, y: last?.position?.y ?? 0 },
      spec: emptySpec(kind),
    };
    setDraft((previous) => ({
      ...previous,
      blocks: [...previous.blocks, block],
      // Appended to the end of the chain when there is one, because that is
      // what "add a block" means on a chain. Anything else is a drag away.
      edges: last ? [...previous.edges, { from: last.id, to: id }] : previous.edges,
    }));
    void navigate({ search: (previous) => ({ ...previous, block: id }), replace: true });
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Pipeline</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Where the rows come from, what shapes them, what a notebook does to them, and what is
            published. Click a block to open its settings or its code; preview 25 rows through the
            whole chain, then run it and save the exact output as a dataset version.
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(window) => void navigate({ search: (previous) => ({ ...previous, window }) })}
        />
      </div>

      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <Badge tone={flowReady.data === false ? 'warning' : undefined}>
          Flow PHP {flowReady.data === false ? 'not running' : 'ready'}
        </Badge>
        <Badge tone={notebooksReady.data === false ? 'warning' : undefined}>
          Notebook runtime {notebooksReady.data === false ? 'not running' : 'ready'}
        </Badge>
        {flowReady.data === false ? <span>start it with `just flow-serve`</span> : null}
        {notebooksReady.data === false ? <span>start it with `just ml-pipeline-serve`</span> : null}
      </div>

      <Card className="flex flex-wrap items-center gap-2 p-3">
        <input
          value={draft.name}
          onChange={(event) => setDraft((previous) => ({ ...previous, name: event.target.value }))}
          placeholder="curation/name"
          className="h-9 w-64 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
        <input
          value={draft.description}
          onChange={(event) =>
            setDraft((previous) => ({ ...previous, description: event.target.value }))
          }
          placeholder="What this curation produces"
          className="h-9 min-w-[16rem] flex-1 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
        <Button
          variant="ghost"
          onClick={() => save.mutate()}
          disabled={busy || !draft.blocks.length}
        >
          {save.isPending ? <Spinner /> : <Save className="h-3.5 w-3.5" />} Save
        </Button>
        <Button variant="outline" onClick={() => load(PII_DETECTION_EXAMPLE)} disabled={busy}>
          <Sparkles className="h-3.5 w-3.5" /> Load the PII example
        </Button>
      </Card>

      {problems.length > 0 ? (
        <Card className="border-danger/40 p-3">
          <p className="text-xs font-medium text-danger">This is not a chain that can run</p>
          <ul className="mt-1 flex list-disc flex-col gap-1 pl-4 text-sm">
            {problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        </Card>
      ) : null}

      {draft.blocks.length === 0 ? (
        <EmptyState
          title="No blocks yet"
          hint="Add a source, or load the PII detection example — a Hugging Face corpus, a Flow PHP transform, a marimo notebook and a view, wired together."
        />
      ) : (
        <PipelineCanvas
          blocks={draft.blocks}
          edges={draft.edges}
          outcomes={outcomes}
          selected={search.block}
          onSelect={(block) =>
            void navigate({ search: (previous) => ({ ...previous, block }), replace: true })
          }
          onMove={(id, position) =>
            setDraft((previous) => ({
              ...previous,
              blocks: previous.blocks.map((block) =>
                block.id === id ? { ...block, position } : block,
              ),
            }))
          }
          onConnect={(edge) =>
            setDraft((previous) => ({
              ...previous,
              // A chain has one edge out of a block and one into it, so
              // connecting replaces rather than adds. Not a rule — the registry
              // owns those — but the editing gesture people expect.
              edges: [
                ...previous.edges.filter(
                  (candidate) => candidate.from !== edge.from && candidate.to !== edge.to,
                ),
                edge,
              ],
            }))
          }
          onDisconnect={(edge) =>
            setDraft((previous) => ({
              ...previous,
              edges: previous.edges.filter(
                (candidate) => candidate.from !== edge.from || candidate.to !== edge.to,
              ),
            }))
          }
          onDelete={(id) => setDraft((previous) => withoutBlock(previous, id))}
        />
      )}

      <div className="flex flex-wrap items-center gap-2">
        {(['source', 'transform', 'notebook', 'view'] as const).map((kind) => (
          <Button key={kind} variant="outline" size="sm" onClick={() => addBlock(kind)}>
            <Plus className="h-3.5 w-3.5" /> {blockLabel(kind)}
          </Button>
        ))}
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <Button
            variant="outline"
            onClick={() => execute.mutate('preview')}
            disabled={busy || !chain || flowReady.data === false}
          >
            {execute.isPending && execute.variables === 'preview' ? (
              <Spinner />
            ) : (
              <Sparkles className="h-3.5 w-3.5" />
            )}{' '}
            Preview 25 rows
          </Button>
          <Button
            onClick={() => execute.mutate('full')}
            disabled={busy || !chain || flowReady.data === false}
          >
            {execute.isPending && execute.variables === 'full' ? (
              <Spinner />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}{' '}
            Run
          </Button>
          <Button
            variant="outline"
            onClick={() => startOnServer.mutate()}
            disabled={busy || !chain}
            title="Compile this pipeline on the server and run it there. The browser may close."
          >
            {startOnServer.isPending ? <Spinner /> : <ServerCog className="h-3.5 w-3.5" />} Run on
            the server
          </Button>
          <Button
            variant="ghost"
            onClick={() => publish.mutate()}
            disabled={busy || !result || !publishTo}
            title={
              publishTo
                ? `Publish as ${publishTo}`
                : 'The view block has no dataset name, so there is nothing to publish to.'
            }
          >
            {publish.isPending ? <Spinner /> : <Upload className="h-3.5 w-3.5" />} Publish
          </Button>
        </div>
      </div>

      {!chain && draft.blocks.length > 0 ? (
        <p className="text-xs text-warning">
          These blocks are not one chain yet, so there is no order to run them in. Connect each
          block to the next one; Save says exactly what is wrong.
        </p>
      ) : null}

      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_26rem]">
        <div className="flex min-w-0 flex-col gap-4">
          {executionId || startOnServer.isPending ? (
            <ManagedRunCard run={managed.data} pending={startOnServer.isPending} />
          ) : null}

          {publish.data ? (
            <Card className="border-success/40 p-3 text-sm">
              Dataset <strong>{publish.data.dataset.name}</strong> saved as version{' '}
              <code className="id">{publish.data.dataset.latest.version.slice(0, 12)}</code> with{' '}
              {publish.data.dataset.latest.row_count} rows, produced by{' '}
              <code className="id">{publish.data.dataset.latest.produced_by?.slice(0, 40)}</code>.
            </Card>
          ) : null}
          {publish.error ? (
            <Card className="border-danger/40 p-3 text-sm text-danger">
              {publish.error.message}
            </Card>
          ) : null}
          {execute.error ? (
            <Card className="border-danger/40 p-3">
              <p className="text-sm text-danger">{execute.error.message}</p>
              {'stderr' in execute.error && typeof execute.error.stderr === 'string' ? (
                <pre className="id mt-2 max-h-48 overflow-auto rounded-md bg-muted/40 p-2 text-[11px]">
                  {execute.error.stderr}
                </pre>
              ) : null}
            </Card>
          ) : null}

          <ResultTable result={result} managed={Boolean(executionId)} />

          {chain ? (
            <Card className="overflow-hidden">
              <div className="border-b border-border p-3 text-xs font-semibold">
                The Flow PHP this chain runs
              </div>
              <pre className="id overflow-x-auto p-3 text-[11px]">{compileFlow(chain)}</pre>
            </Card>
          ) : null}

          <Card className="overflow-hidden">
            <div className="border-b border-border p-3 text-xs font-semibold">Saved pipelines</div>
            {saved.isError ? (
              <p className="p-3 text-xs text-danger">{saved.error.message}</p>
            ) : saved.data?.length ? (
              <div className="divide-y divide-border/50">
                {saved.data.map((pipeline) => (
                  <button
                    key={`${pipeline.name}-${pipeline.revision}`}
                    type="button"
                    onClick={() => load(pipeline)}
                    className="w-full p-3 text-left hover:bg-accent/40"
                  >
                    <p className="truncate text-sm font-medium">{pipeline.name}</p>
                    <p className="mt-0.5 text-[11px] text-muted-foreground">
                      {pipeline.blocks.length} blocks · {pipeline.revision.slice(0, 10)} ·{' '}
                      {new Date(pipeline.saved_at).toLocaleString()}
                    </p>
                  </button>
                ))}
              </div>
            ) : (
              <p className="p-3 text-xs text-muted-foreground">Nothing saved yet.</p>
            )}
          </Card>
        </div>

        {selected ? (
          <BlockInspector
            block={selected}
            onChange={(block) =>
              setDraft((previous) => ({
                ...previous,
                blocks: previous.blocks.map((candidate) =>
                  candidate.id === block.id ? block : candidate,
                ),
              }))
            }
            onDelete={() => {
              setDraft((previous) => withoutBlock(previous, selected.id));
              void navigate({ search: (previous) => ({ ...previous, block: undefined }) });
            }}
          />
        ) : (
          <Card className="p-6 text-center text-sm text-muted-foreground">
            Click a block to open its settings or its code.
          </Card>
        )}
      </div>
    </div>
  );
}

function ResultTable({ result, managed }: { result: PipelineResult | null; managed: boolean }) {
  if (!result) {
    // Two different nothings. With a server run on the page, "nothing run yet"
    // sits directly under a card saying `completed` and reads as a
    // contradiction — so it says which of the two ran, because the rows a
    // managed run produced are in its dataset version and never in this tab.
    return managed ? (
      <EmptyState
        title="Nothing run in this tab"
        hint="The server run above produced its rows into the dataset it publishes. Preview and Run read them here instead, and write nothing."
      />
    ) : (
      <EmptyState
        title="Nothing run yet"
        hint="Preview reads 25 rows through every block and writes nothing."
      />
    );
  }
  // The Flow result's own metadata, with the rows every block after it
  // produced: what the reader is looking at is the *end* of the chain.
  return (
    <FlowResultView
      result={{
        ...result.flow,
        columns: result.columns,
        rows: result.rows,
        row_count: result.rows.length,
      }}
      emptyTitle="The chain produced no rows"
      previewImages
    />
  );
}

function withoutBlock(draft: Draft, id: string): Draft {
  // Deleting from the middle of a chain joins its neighbours, because the
  // alternative is a canvas that quietly stops being runnable every time
  // somebody removes a step.
  const before = draft.edges.find((edge) => edge.to === id);
  const after = draft.edges.find((edge) => edge.from === id);
  const edges = draft.edges.filter((edge) => edge.from !== id && edge.to !== id);
  if (before && after) edges.push({ from: before.from, to: after.to });
  return {
    ...draft,
    blocks: draft.blocks.filter((block) => block.id !== id),
    edges,
  };
}

function nextId(kind: BlockSpec['kind'], blocks: PipelineBlock[]): string {
  for (let index = 1; ; index += 1) {
    const candidate = index === 1 ? kind : `${kind}-${index}`;
    if (!blocks.some((block) => block.id === candidate)) return candidate;
  }
}

function emptySpec(kind: BlockSpec['kind']): BlockSpec {
  switch (kind) {
    case 'source':
      return { kind, dataset: 'runs', arguments: {} };
    case 'transform':
      return { kind, steps: '->limit(100)' };
    case 'notebook':
      return { kind, notebook: 'pii_detection', params: {} };
    case 'view':
      return { kind };
  }
}
