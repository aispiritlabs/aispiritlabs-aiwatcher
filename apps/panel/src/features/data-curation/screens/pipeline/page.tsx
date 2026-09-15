import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getRouteApi } from '@tanstack/react-router';
import {
  Download,
  LayoutGrid,
  Play,
  Save,
  ServerCog,
  ShieldCheck,
  Sparkles,
  Upload,
} from 'lucide-react';
import * as React from 'react';

import {
  getPipelineRevision,
  listPipelines,
  publishDataset,
  publishDatasetSample,
  savePipeline,
  startExecution,
} from '@/api/generated/sdk.gen';
import type {
  CurationPipeline,
  PipelineBlock,
  SaveBlockTemplateRequest,
  SavePipelineRequest,
} from '@/api/generated/types.gen';
import { BlockInspector } from '@/features/data-curation/components/block-inspector';
import { PhaseStrip } from '@/features/data-curation/components/phase-strip';
import { PipelineNotebook } from '@/features/data-curation/components/pipeline-notebook';
import { FlowResultView } from '@/shared/components/flow-preview';
import { ManagedRunCard, useManagedBlocks, useManagedRun } from '@/shared/components/managed-run';
import { SamplePublication } from '@/shared/components/sample-publication';
import { ScheduleCard } from '@/shared/components/schedule-card';

import { rejectionDetails } from '@/shared/lib/rejection';
import { BlockLibrary } from '@/features/data-curation/components/block-library';
import { PipelineCanvas } from '@/features/data-curation/components/pipeline-canvas';
import { phasesOf, type PhaseId } from '@/features/data-curation/lib/phases';
import {
  chainEngine,
  compileQuery,
  followsTheRun,
  managedOutcomes,
  orderOf,
  runPipeline,
  withPinnedNotebooks,
  type BlockResult,
  type PipelineOutcomes,
  type PipelineResult,
} from '@/features/data-curation/lib/pipeline';
import {
  exportBundle,
  importBundle,
  MAX_BUNDLE_BYTES,
} from '@/features/data-curation/lib/pipeline-bundle';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowParam } from '@/shared/components/time-range';
import { Badge, Button, Card, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import { createNotebook, isMlPipelineAvailable } from '@/shared/lib/ml-pipeline';
import { contentFor } from '@/shared/lib/engine-content';
import {
  ENGINE_LABEL,
  isQueryEngineAvailable,
  useQueryEngine,
  writtenElsewhere,
  type QueryEngineName,
} from '@/shared/lib/query';
import { layoutGraph } from '@/shared/lib/workflow-layout';
import { useUnsavedChanges } from '@/shared/lib/unsaved-changes';

const routeApi = getRouteApi('/data-curation/pipeline');

/**
 * The query a chain compiles to, as shown under it: in the language it was
 * written for, or the compiler's own sentence when it would refuse it.
 */
function shownScript(chain: PipelineBlock[], engine: QueryEngineName): string {
  try {
    return compileQuery(chain, engine);
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

type Draft = {
  name: string;
  description: string;
  blocks: PipelineBlock[];
  edges: { from: string; to: string }[];
};

/** What the canvas was last known to be, and under which content address. */
type Pinned = {
  name: string;
  revision: string;
  blocks: PipelineBlock[];
  edges: { from: string; to: string }[];
};

const EMPTY_DRAFT: Draft = {
  name: 'curation/untitled',
  description: '',
  blocks: [],
  edges: [],
};

const contextOf = (search: { name?: string; revision?: string; draft?: string }) =>
  JSON.stringify([search.name, search.revision, search.draft]);
type LoadedContext = { draft: Draft; savedDraft: Draft; pinned?: Pinned };

function pipelineContext(pipeline: CurationPipeline | SavePipelineRequest): LoadedContext {
  const draft = { name: pipeline.name, description: pipeline.description ?? '',
    blocks: pipeline.blocks, edges: pipeline.edges ?? [] };
  return { draft, savedDraft: 'revision' in pipeline ? draft : EMPTY_DRAFT,
    pinned: 'revision' in pipeline ? { name: pipeline.name, revision: pipeline.revision,
      blocks: pipeline.blocks, edges: pipeline.edges } : undefined };
}

export function PipelinePage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const queryClient = useQueryClient();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const [draft, setDraft] = React.useState<Draft>(EMPTY_DRAFT);
  const [savedDraft, setSavedDraft] = React.useState<Draft>(EMPTY_DRAFT);
  const dirty = JSON.stringify(draft) !== JSON.stringify(savedDraft);
  const context = contextOf(search);
  const activeContext = React.useRef(context);
  const generation = React.useRef(0);
  const historyContexts = React.useRef(new Map<string, LoadedContext>());
  const [contextIssue, setContextIssue] = React.useState('');
  const [editorGeneration, setEditorGeneration] = React.useState(0);
  const mounted = React.useRef(true);
  React.useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  // The revision this canvas was last loaded or saved at, with the blocks it
  // held then. A managed run's states may be drawn over the draft only while
  // the two still agree — see `atRevision`.
  const [pinned, setPinned] = React.useState<Pinned>();
  const [scheduleDirty, setScheduleDirty] = React.useState(false);
  const [scheduleEditor, setScheduleEditor] = React.useState(0);
  const [outcomes, setOutcomes] = React.useState<PipelineOutcomes>({});
  const [result, setResult] = React.useState<PipelineResult | null>(null);
  const notebookMode = search.view === 'notebook';
  const [cellResults, setCellResults] = React.useState<Record<string, BlockResult>>({});
  const [dirtyEditors, setDirtyEditors] = React.useState<Record<string, boolean>>({});
  const reportDirty = React.useCallback((id: string, dirty: boolean) => {
    setDirtyEditors((previous) =>
      previous[id] === dirty ? previous : { ...previous, [id]: dirty },
    );
  }, []);
  const hasUnsavedCode = Object.values(dirtyEditors).some(Boolean);
  const executionSignature = JSON.stringify([
    context,
    draft.blocks.map((block) => [block.id, block.spec]),
    draft.edges,
    windowSeconds,
  ]);
  const currentSignature = React.useRef(executionSignature);
  currentSignature.current = executionSignature;
  React.useEffect(() => {
    setResult(null);
    setOutcomes({});
    setCellResults({});
  }, [executionSignature]);
  const [problems, setProblems] = React.useState<string[]>([]);

  const flowReady = useQuery({
    queryKey: ['flow', 'available'],
    queryFn: isQueryEngineAvailable,
    refetchInterval: 15_000,
  });
  // The engine this deployment runs (AW-3): the language the browser compiles a
  // chain to, and the one a chain written for another is shown in, not run.
  const deployed = useQueryEngine().data?.engine ?? undefined;
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

  const linkedRevision = useQuery({
    queryKey: ['curation-pipeline-revision', search.name, search.revision],
    enabled: Boolean(search.name && search.revision && !search.draft),
    queryFn: async () => {
      const response = await getPipelineRevision({ path: { name: search.name!, revision: search.revision! } });
      if (response.response?.status === 404) return null;
      if (!response.data) throw new Error('Could not load the linked pipeline revision.');
      return response.data;
    },
  });

  // Restore a loaded context on every history transition. Keep local imports
  // for this mounted page; a missing local draft after reload is explicit.
  const hydrated = React.useRef(false);
  React.useEffect(() => {
    const changed = activeContext.current !== context;
    if (changed) {
      activeContext.current = context;
      generation.current += 1;
      hydrated.current = false;
      setDraft(EMPTY_DRAFT); setSavedDraft(EMPTY_DRAFT); setPinned(undefined);
      setScheduleEditor((previous) => previous + 1);
      setEditorGeneration((previous) => previous + 1);
      setDirtyEditors({}); setScheduleDirty(false);
      setOutcomes({}); setCellResults({}); setResult(null); setProblems([]);
      setContextIssue('');
    }
    if (hydrated.current) return;
    const cached = historyContexts.current.get(context);
    const pipeline = linkedRevision.data ?? saved.data?.find((candidate) => candidate.name === search.name &&
      (!search.revision || candidate.revision === search.revision));
    const restored = cached ?? (!search.draft && pipeline ? pipelineContext(pipeline) : undefined);
    if (!restored) {
      if (search.draft) setContextIssue('This local flow is no longer available. Import it again or open a saved pipeline.');
      else if (search.revision ? linkedRevision.isError : search.name && saved.isError)
        setContextIssue('Could not load the linked pipeline. Retry loading it.');
      else if (search.revision ? linkedRevision.data === null : search.name && saved.data)
        setContextIssue('The linked pipeline revision is unavailable. Open an available saved pipeline.');
      return;
    }
    // The initial request may finish after somebody starts an empty draft.
    if (dirty && !changed && !cached) return;
    hydrated.current = true;
    setContextIssue('');
    setDraft(restored.draft); setSavedDraft(restored.savedDraft); setPinned(restored.pinned);
    historyContexts.current.set(context, restored);
    if (!search.revision && restored.pinned) {
      const next = { ...search, revision: restored.pinned.revision };
      activeContext.current = contextOf(next);
      historyContexts.current.set(activeContext.current, restored);
      void navigate({ search: next, replace: true, ignoreBlocker: true });
    }
  }, [context, saved.data, saved.isError, linkedRevision.data, linkedRevision.isError]);

  /**
   * The revision this canvas *is*, or `undefined` once somebody has changed it.
   *
   * A revision is a content address the server computes over the whole authored
   * request, positions included, so the browser cannot work out what the draft
   * would be saved as. What it can do is notice that the draft is still exactly
   * what it was loaded as — and an edit that a comparison like this reads as a
   * change when it is not costs a "you have edited this" line, which is the
   * safe direction to be wrong in.
   */
  const atRevision = React.useMemo(() => {
    if (!pinned) return undefined;
    const same =
      JSON.stringify(pinned.blocks) === JSON.stringify(draft.blocks) &&
      JSON.stringify(pinned.edges) === JSON.stringify(draft.edges);
    return same ? pinned.revision : undefined;
  }, [pinned, draft.blocks, draft.edges]);

  const chain = React.useMemo(() => orderOf(draft.blocks, draft.edges), [draft]);
  const selected = draft.blocks.find((block) => block.id === search.block);
  // Derived every render rather than held: a phase is a fact about the blocks,
  // and a copy of it in state is a copy that goes stale on the next edit.
  const phases = React.useMemo(
    () => phasesOf(draft.blocks, draft.edges),
    [draft.blocks, draft.edges],
  );

  /*
   * The same draft with the blocks laid out along the chain.
   *
   * Positions are authored — they ride in the definition revision — so this is
   * an *edit*, not a view setting: it lands in the draft and is only kept if
   * somebody saves. That is also why it is a button rather than something the
   * canvas does on load; a layout that rearranged itself would throw away an
   * arrangement somebody made on purpose.
   *
   * `layoutGraph` is the workflow graph's own layering, which puts a chain on
   * one line and stacks siblings. A ten-block chain therefore stops wrapping
   * into two rows joined by a diagonal across the canvas.
   */
  const [fitSignal, setFitSignal] = React.useState(0);

  const collapsed = React.useMemo(
    () => new Set((search.fold ?? '').split(',').filter(Boolean) as PhaseId[]),
    [search.fold],
  );
  const toggleCollapsed = React.useCallback(
    (phase: PhaseId) => {
      const next = new Set(collapsed);
      if (!next.delete(phase)) next.add(phase);
      void navigate({
        search: (previous) => ({
          ...previous,
          fold: next.size > 0 ? [...next].join(',') : undefined,
        }),
        replace: true,
      });
    },
    [collapsed, navigate],
  );

  const tidied = React.useMemo(() => {
    const placed = new Map(
      layoutGraph(
        draft.blocks.map((block) => block.id),
        draft.edges,
      ).map((node) => [node.id, node.position]),
    );
    return {
      ...draft,
      blocks: draft.blocks.map((block) => ({
        ...block,
        position: placed.get(block.id) ?? block.position,
      })),
    };
  }, [draft]);
  const view = chain?.find((block) => block.spec.kind === 'view');
  const publishTo = view?.spec.kind === 'view' ? view.spec.dataset : undefined;

  const load = (pipeline: CurationPipeline | SavePipelineRequest, confirmed = false, view?: 'notebook') => {
    if (!confirmed && !confirmDiscard()) return;
    generation.current += 1;
    setEditorGeneration((previous) => previous + 1);
    setDirtyEditors({}); setScheduleDirty(false);
    setContextIssue('');
    setScheduleEditor((previous) => previous + 1);
    hydrated.current = true;
    const restored = pipelineContext(pipeline);
    setPinned(restored.pinned);
    setDraft(restored.draft);
    setSavedDraft(restored.savedDraft);
    const next = { ...search, name: pipeline.name,
      revision: 'revision' in pipeline ? pipeline.revision : undefined,
      draft: 'revision' in pipeline ? undefined : crypto.randomUUID(),
      block: undefined, execution: undefined, view: view ?? search.view };
    activeContext.current = contextOf(next);
    historyContexts.current.set(activeContext.current, restored);
    setOutcomes({});
    setResult(null);
    setProblems([]);
    void navigate({ search: next, ignoreBlocker: true });
  };

  const save = useMutation({
    mutationFn: async () => {
      const contextGeneration = generation.current;
      if (scheduleDirty && pinned?.name !== draft.name) {
        throw new Error('Save or discard the schedule changes before saving the pipeline under a different name.');
      }
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
      return { ...response.data, submitted: draft, contextGeneration };
    },
    onSuccess: (stored) => {
      void queryClient.invalidateQueries({ queryKey: ['curation-pipelines'] });
      if (!mounted.current || generation.current !== stored.contextGeneration) return;
      setProblems([]);
      // The stored blocks rather than the draft's: saving pins each notebook's
      // revision, so what came back is what this canvas now is.
      setDraft((previous) => ({ ...previous,
        blocks: JSON.stringify(previous.blocks) === JSON.stringify(stored.submitted.blocks)
          ? stored.pipeline.blocks : previous.blocks,
      }));
      setSavedDraft({ name: stored.pipeline.name, description: stored.pipeline.description ?? '',
        blocks: stored.pipeline.blocks, edges: stored.pipeline.edges ?? [] });
      setPinned({
        name: stored.pipeline.name,
        revision: stored.pipeline.revision,
        blocks: stored.pipeline.blocks,
        edges: stored.pipeline.edges,
      });
      const restored = pipelineContext(stored.pipeline);
      const next = { name: stored.pipeline.name, revision: stored.pipeline.revision, draft: undefined };
      activeContext.current = contextOf(next);
      historyContexts.current.set(activeContext.current, restored);
      void navigate({ search: (previous) => ({ ...previous, ...next }), replace: true, ignoreBlocker: true });
    },
    // Every reason at once, from the registry, which is the only place that
    // decides whether a chain is runnable.
    onMutate: () => generation.current,
    onError: (error, _variables, contextGeneration) => {
      if (mounted.current && generation.current === contextGeneration) setProblems(rejectionDetails(error));
    },
  });

  // Content written for another engine is shown, not run (AW-3): a chain's
  // engine is its transforms', and a source alone runs on the deployed one.
  const written = chain ? chainEngine(chain) : undefined;
  const foreign = writtenElsewhere(written ?? deployed ?? 'flow', deployed);

  const execute = useMutation({
    mutationFn: async ({ mode, until }: { mode: 'preview' | 'full'; until?: string }) => {
      if (!chain) throw new Error('Connect the blocks into one chain first.');
      setOutcomes({});
      setCellResults({});
      setResult(null);
      const signature = currentSignature.current;
      const contextGeneration = generation.current;
      const end = until ? chain.findIndex((block) => block.id === until) : chain.length - 1;
      if (end < 0) throw new Error('This cell is no longer in the flow.');
      const produced = await runPipeline({
        chain: chain.slice(0, end + 1),
        inspectBlocks: notebookMode || Boolean(until),
        onBlockResult: (id, value) => {
          if (mounted.current && generation.current === contextGeneration && currentSignature.current === signature)
            setCellResults((previous) => ({ ...previous, [id]: value }));
        },
        mode,
        engine: deployed ?? 'flow',
        windowSeconds: windowParam(windowSeconds),
        onOutcome: (id, outcome) => {
          if (mounted.current && generation.current === contextGeneration && currentSignature.current === signature)
            setOutcomes((previous) => ({ ...previous, [id]: outcome }));
        },
      });
      return { produced, signature, contextGeneration, complete: end === chain.length - 1 };
    },
    onSuccess: ({ produced, signature, contextGeneration, complete }) => {
      if (complete && mounted.current && generation.current === contextGeneration && currentSignature.current === signature) setResult(produced);
    },
  });

  const publish = useMutation({
    mutationFn: async (asSample: boolean) => {
      if (!result || (asSample ? !result.sample : !result.complete)) {
        throw new Error('Ordinary publication requires a complete run. Limited output must be published explicitly as a sample.');
      }
      if (!publishTo) throw new Error('The view block has no dataset name.');
      // Saved first, always: the version records the chain that made it, and a
      // chain that was never saved is a reference to nothing. The Flow script
      // alone does not describe this execution — a notebook ran after it.
      // Persist the executed notebook revisions, not the current mutable heads.
      // Keep this snapshot separate from the editor so a refused publication
      // leaves its result available for retry.
      const definition = await savePipeline({ body: { ...draft, blocks: draft.blocks.map((block) =>
        block.spec.kind === 'notebook' && result.notebookRevisions[block.id]
          ? { ...block, spec: { ...block.spec, revision: result.notebookRevisions[block.id] } } : block),
      } });
      if (!definition.data) throw new Error('Could not save the executed pipeline definition.');
      const stored = definition.data;
      void queryClient.invalidateQueries({ queryKey: ['curation-pipelines'] });
      const response = await (asSample ? publishDatasetSample : publishDataset)({
        body: {
          name: asSample ? `${publishTo}/samples` : publishTo,
          sample: asSample ? result.sample : undefined,
          description: draft.description,
          pipeline: result.script,
          engine: result.engine,
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

  React.useEffect(() => { publish.reset(); }, [execute.submittedAt]);

  // The run the server owns, if this page is following one. Held by id rather
  // than by object: the projection is re-read from the store on every frame,
  // and a copy here would be the stale one.
  const executionId = search.execution;
  const managed = useManagedRun(executionId);
  const authored = useManagedBlocks(executionId);

  /**
   * Whether the canvas on screen is the canvas that run compiled.
   *
   * `undefined` while there is no managed run to compare against. Otherwise
   * the run's own pinned revision against this draft's — and the states below
   * are drawn only when they are the same, because lighting a block that was
   * added after the run is claiming an outcome for something that never ran.
   */
  const following = followsTheRun(atRevision, authored.data?.definition_revision);

  /**
   * What the canvas draws: the managed run's step states while it is following
   * one, and the browser's own run otherwise.
   *
   * Never merged. Two runs of one chain produce two sets of outcomes, and a box
   * showing the ad-hoc preview's row count beside a managed run's `running` is
   * two answers to one question.
   */
  const canvasOutcomes = React.useMemo(() => {
    if (!following || !managed.data || !authored.data) return outcomes;
    return managedOutcomes(managed.data.execution.steps, authored.data.steps);
  }, [following, managed.data, authored.data, outcomes]);

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
      return { ...response.data, contextGeneration: stored.contextGeneration };
    },
    onSuccess: (accepted) => {
      if (!mounted.current || generation.current !== accepted.contextGeneration) return;
      void navigate({
        search: (previous) => ({ ...previous, execution: accepted.execution.execution_id }),
        replace: true,
      });
    },
    onMutate: () => generation.current,
    onError: (error, _variables, contextGeneration) => {
      if (mounted.current && generation.current === contextGeneration) setProblems(rejectionDetails(error));
    },
  });

  const importInput = React.useRef<HTMLInputElement>(null);
  const [transferNotice, setTransferNotice] = React.useState('');
  const importFlow = useMutation({
    mutationFn: async (file: File) => {
      if (file.size > MAX_BUNDLE_BYTES) throw new Error('A flow bundle must be at most 8 MiB.');
      const contextGeneration = generation.current;
      return { pipeline: await importBundle(await file.text()), contextGeneration };
    },
    onSuccess: ({ pipeline, contextGeneration }) => {
      if (!mounted.current || generation.current !== contextGeneration) return;
      load(pipeline, true);
      setTransferNotice(
        'Flow and Python sources imported. Review the code and parameters, then Preview or Save.',
      );
      void queryClient.invalidateQueries({ queryKey: ['ml-pipeline'] });
    },
    onMutate: () => generation.current,
    onError: (error, _variables, contextGeneration) => {
      if (mounted.current && generation.current === contextGeneration) setProblems(rejectionDetails(error));
    },
  });
  const exportFlow = useMutation({
    mutationFn: async () => exportBundle(draft),
    onSuccess: (bundle) => {
      const url = URL.createObjectURL(
        new Blob([JSON.stringify(bundle, null, 2) + '\n'], { type: 'application/json' }),
      );
      const link = document.createElement('a');
      link.href = url;
      link.download = `${draft.name.replace(/[^a-zA-Z0-9_-]/g, '-')}.flow.json`;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      setTransferNotice('Exported the flow, parameters, layout and exact Python source revisions.');
    },
    onMutate: () => generation.current,
    onError: (error, _variables, contextGeneration) => {
      if (mounted.current && generation.current === contextGeneration) setProblems(rejectionDetails(error));
    },
  });
  const newNotebookFlow = useMutation({
    mutationFn: async () => {
      const contextGeneration = generation.current;
      return { notebook: await createNotebook(), contextGeneration };
    },
    onSuccess: ({ notebook, contextGeneration }) => {
      if (!mounted.current || generation.current !== contextGeneration) return;
      const name = `curation/notebook-${crypto.randomUUID().slice(0, 8)}`;
      load({
        name,
        description: '',
        blocks: [
          {
            id: 'source',
            title: 'Import data',
            position: { x: 0, y: 0 },
            spec: { kind: 'source', dataset: 'runs', arguments: {} },
          },
          {
            id: 'prepare',
            title: 'Prepare data',
            position: { x: 270, y: 0 },
            spec: {
              kind: 'transform',
              steps: contentFor(deployed)?.newTransform ?? 'df.limit(100)',
              engine: deployed ?? 'flow',
            },
          },
          {
            id: 'python',
            title: 'My Python code',
            position: { x: 540, y: 0 },
            spec: {
              kind: 'notebook',
              notebook: notebook.name,
              revision: notebook.revision,
              params: {},
            },
          },
          {
            id: 'publish',
            title: 'Save dataset',
            position: { x: 810, y: 0 },
            spec: { kind: 'view', dataset: name },
          },
        ],
        edges: [
          { from: 'source', to: 'prepare' },
          { from: 'prepare', to: 'python' },
          { from: 'python', to: 'publish' },
        ],
      }, true, 'notebook');
      void queryClient.invalidateQueries({ queryKey: ['ml-pipeline'] });
    },
    onMutate: () => generation.current,
    onError: (error, _variables, contextGeneration) => {
      if (mounted.current && generation.current === contextGeneration) setProblems(rejectionDetails(error));
    },
  });
  const [libraryBusy, setLibraryBusy] = React.useState(false);
  // Not a library entry: the library holds solutions somebody on this
  // installation published, and a gate has nothing to share — no code, no
  // parameters, and a question that belongs to the chain it stops. So it is
  // the page's own affordance, put beside the library because that is where a
  // block gets added. It starts at the floor; the inspector raises it.
  const approvalTemplate = (): SaveBlockTemplateRequest => ({
    id: 'approval',
    title: 'Approval',
    description: '',
    tags: [],
    spec: {
      kind: 'approval',
      prompt: '',
      role: 'editor',
      choices: ['approve', 'reject'],
    },
  });
  const busy =
    newNotebookFlow.isPending ||
    libraryBusy ||
    execute.isPending ||
    save.isPending ||
    publish.isPending ||
    startOnServer.isPending ||
    importFlow.isPending ||
    exportFlow.isPending;

  React.useEffect(() => {
    save.reset(); execute.reset(); publish.reset(); startOnServer.reset();
    importFlow.reset(); exportFlow.reset(); newNotebookFlow.reset();
    setTransferNotice('');
  }, [editorGeneration]);

  const contextLoading = !hydrated.current && Boolean(search.name) && !search.draft &&
    (search.revision ? linkedRevision.isPending : saved.isPending);
  const locked = busy || hasUnsavedCode || contextLoading;
  const renameNeedsSchedule = scheduleDirty && pinned?.name !== draft.name;
  const confirmDiscard = useUnsavedChanges({
    dirty: dirty || hasUnsavedCode || scheduleDirty,
    pending: busy,
    message: 'This pipeline or its schedule has unsaved changes.',
    losesDraft: ({ next }) => next.routeId !== '/data-curation/pipeline' || contextOf(next.search) !== context ||
      (hasUnsavedCode && ((next.search.view === 'notebook') !== notebookMode ||
        (!notebookMode && next.search.block !== search.block))),
  });

  const deleteBlock = (id: string) => {
    if (busy || (hasUnsavedCode && !confirmDiscard())) return;
    setDraft((previous) => withoutBlock(previous, id));
  };

  const addBlock = (template: SaveBlockTemplateRequest) => {
    const id = nextId(template.id, draft.blocks);
    // PHP preparation precedes Python; Python processing precedes publication.
    const tail = chain?.at(-1);
    const publishTail =
      (template.spec.kind === 'transform'
        ? chain?.find((item) => item.spec.kind === 'notebook' || item.spec.kind === 'view')
        : undefined) ?? (tail?.spec.kind === 'view' ? tail : undefined);
    const successorIndex = publishTail
      ? chain?.findIndex((item) => item.id === publishTail.id)
      : -1;
    const last =
      publishTail && successorIndex !== undefined && successorIndex > 0
        ? chain?.[successorIndex - 1]
        : publishTail
          ? undefined
          : (tail ?? draft.blocks.at(-1));
    const block: PipelineBlock = {
      id,
      title: template.title,
      position: { x: (last?.position?.x ?? -300) + 300, y: last?.position?.y ?? 0 },
      spec: structuredClone(template.spec),
    };
    setDraft((previous) => ({
      ...previous,
      blocks: [...previous.blocks, block],
      edges: [
        ...previous.edges.filter((edge) => !(publishTail && edge.to === publishTail.id)),
        ...(last ? [{ from: last.id, to: id }] : []),
        ...(publishTail ? [{ from: id, to: publishTail.id }] : []),
      ],
    }));
    void navigate({ search: (previous) => ({ ...previous, block: id }), replace: true });
  };

  return (
    <div className="flex flex-col gap-4" inert={importFlow.isPending || newNotebookFlow.isPending}
      aria-busy={importFlow.isPending || newNotebookFlow.isPending}>
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
          {deployed ? ENGINE_LABEL[deployed] : 'Query engine'}{' '}
          {flowReady.data === false ? 'not running' : 'ready'}
        </Badge>
        <Badge tone={notebooksReady.data === false ? 'warning' : undefined}>
          Notebook runtime {notebooksReady.data === false ? 'not running' : 'ready'}
        </Badge>
        {flowReady.data === false ? <span>start it with `just query-serve`</span> : null}
        {notebooksReady.data === false ? <span>start it with `just ml-pipeline-serve`</span> : null}
      </div>

      <Card className="flex flex-wrap items-center gap-2 p-3">
        <input
          aria-label="Pipeline name"
          disabled={contextLoading}
          value={draft.name}
          onChange={(event) => setDraft((previous) => ({ ...previous, name: event.target.value }))}
          placeholder="curation/name"
          className="h-9 w-64 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
        <input
          aria-label="Pipeline description"
          disabled={contextLoading}
          value={draft.description}
          onChange={(event) =>
            setDraft((previous) => ({ ...previous, description: event.target.value }))
          }
          placeholder="What this curation produces"
          className="h-9 min-w-[16rem] flex-1 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
        <Button
          aria-label="Save pipeline"
          variant="ghost"
          onClick={() => save.mutate()}
          disabled={locked || renameNeedsSchedule || !draft.blocks.length}
        >
          {save.isPending ? <Spinner /> : <Save className="h-3.5 w-3.5" />} Save
        </Button>
      </Card>

      {dirty ? <p role="status" className="text-xs text-warning">Unsaved pipeline changes.</p> : null}
      {renameNeedsSchedule ? <p role="status" className="text-xs text-warning">
        Save or discard the schedule changes before saving the pipeline under a different name.
      </p> : null}

      <div className="flex flex-wrap items-center gap-2">
        <input
          ref={importInput}
          type="file"
          accept=".json,application/json"
          aria-label="Import flow file"
          className="hidden"
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = '';
            if (file && confirmDiscard()) importFlow.mutate(file);
          }}
        />
        <Button variant="outline" disabled={locked} onClick={() => importInput.current?.click()}>
          {importFlow.isPending ? <Spinner /> : <Upload className="h-3.5 w-3.5" />} Import flow
        </Button>
        <Button variant="outline" disabled={locked || !chain} onClick={() => exportFlow.mutate()}>
          {exportFlow.isPending ? <Spinner /> : <Download className="h-3.5 w-3.5" />} Export flow
          with code
        </Button>
        <span className="text-xs text-muted-foreground">
          {transferNotice ||
            'A .flow.json file includes blocks, connections, parameters and Python sources.'}
        </span>
      </div>

      <Card className="overflow-hidden">
        <div className="border-b border-border p-3 text-xs font-semibold">Saved pipelines</div>
        {saved.isError ? (
          <p className="p-3 text-xs text-danger">{saved.error.message}</p>
        ) : saved.data?.length ? (
          <div className="grid max-h-56 overflow-auto sm:grid-cols-2 lg:grid-cols-3">
            {saved.data.map((pipeline) => (
              <button
                key={`${pipeline.name}-${pipeline.revision}`}
                type="button"
                onClick={() => load(pipeline)}
                disabled={busy}
                className="w-full p-3 text-left hover:bg-accent/40 disabled:opacity-50"
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

      {contextLoading ? <p role="status" className="text-sm text-muted-foreground">Loading pipeline…</p> : null}
      {contextIssue ? <Card role="alert" className="border-warning/40 p-3 text-sm">
        {contextIssue}
        {saved.isError || linkedRevision.isError ? <Button variant="outline" onClick={() => void (search.revision ? linkedRevision.refetch() : saved.refetch())}>Retry pipeline</Button> : null}
      </Card> : null}

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

      {following === false ? (
        // Not a warning about something being wrong: an edited draft is the
        // ordinary state of somebody working. What it may not do is borrow the
        // run's outcomes, because those belong to blocks that are not these.
        <Card className="border-warning/40 p-3 text-sm">
          <p className="font-medium">This canvas is not what the run below compiled</p>
          <p className="mt-1 text-xs text-muted-foreground">
            The run pinned revision {authored.data?.definition_revision.slice(0, 10)}, and this
            draft has moved since. Its steps are listed under the run itself; the boxes here stay
            unlit rather than claiming an outcome that belongs to a different drawing. Save this
            pipeline and start it again to follow it on the canvas.
          </p>
        </Card>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant={notebookMode ? 'outline' : 'default'}
          disabled={busy}
          onClick={() => void navigate({ search: (previous) => ({ ...previous, view: 'canvas' }) })}
        >
          Canvas view
        </Button>
        <Button
          variant={notebookMode ? 'default' : 'outline'}
          disabled={busy}
          onClick={() =>
            void navigate({ search: (previous) => ({ ...previous, view: 'notebook' }) })
          }
        >
          Notebook view
        </Button>
        <Button
          variant="outline"
          disabled={locked || notebooksReady.data === false}
          onClick={() => { if (confirmDiscard()) newNotebookFlow.mutate(); }}
        >
          New notebook flow
        </Button>
        {hasUnsavedCode ? (
          <p className="text-xs text-warning">
            Save notebook code and fix settings before running or exporting. Changing views asks before discarding edits.
          </p>
        ) : null}
      </div>
      {draft.blocks.length === 0 ? (
        <EmptyState
          title="No blocks yet"
          hint="Add a source from the public solutions library, import a flow, or open a saved pipeline."
        />
      ) : notebookMode ? null : (
        <>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <PhaseStrip
              phases={phases}
              collapsed={collapsed}
              onToggleCollapsed={toggleCollapsed}
              selected={search.phase}
              onSelect={(phase) =>
                void navigate({
                  search: (previous) => ({ ...previous, phase }),
                  replace: true,
                })
              }
            />
            <Button
              variant="outline"
              disabled={locked}
              title="Lay the blocks out along the chain. This edits positions, so it is only kept if you save."
              onClick={() => {
                setDraft(tidied);
                setFitSignal((previous) => previous + 1);
              }}
            >
              <LayoutGrid className="h-3.5 w-3.5" />
              Tidy layout
            </Button>
          </div>
          <PipelineCanvas
            fitSignal={fitSignal}
            collapsed={collapsed}
            onExpand={toggleCollapsed}
            blocks={draft.blocks}
            edges={draft.edges}
            outcomes={canvasOutcomes}
            selected={search.block}
            phase={search.phase}
            reach={search.reach}
            onReach={(mode) =>
              void navigate({ search: (previous) => ({ ...previous, reach: mode }), replace: true })
            }
            onSelect={(block) => {
              if (!busy)
                void navigate({ search: (previous) => ({ ...previous, block }), replace: true });
            }}
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
            onDelete={deleteBlock}
          />
        </>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <BlockLibrary
          disabled={locked}
          selected={selected}
          onAdd={addBlock}
          onBusyChange={setLibraryBusy}
        />
        <Button
          variant="outline"
          disabled={locked}
          onClick={() => addBlock(approvalTemplate())}
          title="Stop a managed run here until somebody answers. The browser-driven run walks past it."
        >
          <ShieldCheck className="h-3.5 w-3.5" /> Add an approval
        </Button>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <Button
            variant="outline"
            onClick={() => execute.mutate({ mode: 'preview' })}
            disabled={locked || !chain || flowReady.data === false || Boolean(foreign)}
          >
            {execute.isPending && execute.variables?.mode === 'preview' ? (
              <Spinner />
            ) : (
              <Sparkles className="h-3.5 w-3.5" />
            )}{' '}
            Preview 25 rows
          </Button>
          <Button
            onClick={() => execute.mutate({ mode: 'full' })}
            disabled={locked || !chain || flowReady.data === false || Boolean(foreign)}
          >
            {execute.isPending && execute.variables?.mode === 'full' ? (
              <Spinner />
            ) : (
              <Play className="h-3.5 w-3.5" />
            )}{' '}
            Run
          </Button>
          <Button
            variant="outline"
            onClick={() => startOnServer.mutate()}
            disabled={locked || renameNeedsSchedule || !chain}
            title="Compile this pipeline on the server and run it there. The browser may close."
          >
            {startOnServer.isPending ? <Spinner /> : <ServerCog className="h-3.5 w-3.5" />} Run on
            the server
          </Button>
          <Button
            variant="ghost"
            onClick={() => publish.mutate(false)}
            disabled={locked || renameNeedsSchedule || !result?.complete || !publishTo}
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

      {result?.sample && publishTo ? <SamplePublication
        name={`${publishTo}/samples`} rowCount={result.rows.length} sample={result.sample}
        pending={publish.isPending} disabled={locked || renameNeedsSchedule}
        error={publish.variables === true ? publish.error : null}
        published={publish.variables === true ? publish.data : undefined}
        onPublish={() => publish.mutate(true)}
      /> : null}
      {result && !result.complete && (
        <p role="status" className="text-sm text-warning">
          {result.mode === 'preview' ? 'Preview sample' : 'Truncated result'} — ordinary publication is unavailable.
          Publish an explicit sample above, run the complete pipeline, or use a server run for larger datasets.
        </p>
      )}
      {foreign ? (
        <p className="text-xs text-warning">
          {foreign} Run on the server is refused for the same reason, by name.
        </p>
      ) : null}

      {!chain && draft.blocks.length > 0 ? (
        <p className="text-xs text-warning">
          These blocks are not one chain yet, so there is no order to run them in. Connect each
          block to the next one; Save says exactly what is wrong.
        </p>
      ) : null}

      {notebookMode && chain ? (
        <PipelineNotebook
          key={editorGeneration}
          chain={chain}
          outcomes={outcomes}
          results={cellResults}
          busy={busy}
          locked={locked}
          onChange={(block) =>
            setDraft((previous) => ({
              ...previous,
              blocks: previous.blocks.map((candidate) =>
                candidate.id === block.id ? block : candidate,
              ),
            }))
          }
          onDelete={deleteBlock}
          onRunTo={(id) => execute.mutate({ mode: 'preview', until: id })}
          onDirtyChange={reportDirty}
          onPublish={() => publish.mutate(false)}
          canPublish={Boolean(result?.complete && publishTo && !renameNeedsSchedule)}
        />
      ) : null}
      {notebookMode && execute.error ? (
        <p role="alert" className="text-sm text-danger">
          {execute.error.message}
        </p>
      ) : null}
      {notebookMode && publish.error && publish.variables !== true ? (
        <p role="alert" className="text-sm text-danger">
          {publish.error.message}
        </p>
      ) : null}
      {notebookMode && publish.data && publish.variables !== true ? (
        <p role="status" className="text-sm text-primary">
          {publish.data.dataset.latest.sample ? 'Sample' : 'Dataset'} {publish.data.dataset.name} saved with {publish.data.dataset.latest.row_count}{' '}
          rows.
        </p>
      ) : null}
      <div
        className={
          notebookMode ? 'hidden' : 'grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(26rem,40%)]'
        }
      >
        <div className="flex min-w-0 flex-col gap-4">
          {/* Beside the managed run rather than under the canvas: both answer
              "what does the server do with this", and a schedule read next to
              the run it produces is how somebody checks it did. */}
          <ScheduleCard key={`${pinned?.name ?? 'unsaved'}:${scheduleEditor}`} name={pinned?.name}
            saved={Boolean(pinned)} onDirtyChange={setScheduleDirty} />

          {executionId || startOnServer.isPending ? (
            <ManagedRunCard
              run={managed.data ?? undefined}
              executionId={executionId}
              pending={startOnServer.isPending}
              // `null` is the server saying there is no such run; an error is
              // the server not saying anything usable. Only the first is
              // absence, and only the first offers to forget the link.
              missing={managed.data === null}
              failure={managed.error}
              onForget={() =>
                void navigate({
                  search: (previous) => ({ ...previous, execution: undefined }),
                  replace: true,
                })
              }
            />
          ) : null}

          {publish.data && publish.variables !== true ? (
            <Card className="border-success/40 p-3 text-sm">
              {publish.data.dataset.latest.sample ? 'Sample' : 'Dataset'} <strong>{publish.data.dataset.name}</strong> saved as version{' '}
              <code className="id">{publish.data.dataset.latest.version.slice(0, 12)}</code> with{' '}
              {publish.data.dataset.latest.row_count} rows, produced by{' '}
              <code className="id">{publish.data.dataset.latest.produced_by?.slice(0, 40)}</code>.
            </Card>
          ) : null}
          {publish.error && publish.variables !== true ? (
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

          {result?.notebooks
            .filter((run) => run.stdout.trim())
            .map((run, index) => (
              <Card key={`${run.notebook}-${index}`} className="overflow-hidden">
                <div className="border-b border-border p-3 text-xs font-semibold">
                  {draft.blocks.find(
                    (block) =>
                      block.spec.kind === 'notebook' && block.spec.notebook === run.notebook,
                  )?.title ?? 'Python report'}
                </div>
                <pre className="id max-h-48 overflow-auto whitespace-pre-wrap p-3 text-xs">
                  {run.stdout}
                </pre>
              </Card>
            ))}
          <ResultTable result={result} managed={Boolean(executionId)} />

          {chain ? (
            <Card className="overflow-hidden">
              <div className="border-b border-border p-3 text-xs font-semibold">
                The {ENGINE_LABEL[written ?? deployed ?? 'flow']} this chain runs
              </div>
              <pre className="id overflow-x-auto p-3 text-[11px]">
                {shownScript(chain, written ?? deployed ?? 'flow')}
              </pre>
            </Card>
          ) : null}
        </div>

        {!notebookMode && selected ? (
          <BlockInspector
            key={`${editorGeneration}:${selected.id}`}
            block={selected}
            disabled={busy}
            onDirtyChange={reportDirty}
            onChange={(block) =>
              setDraft((previous) => ({
                ...previous,
                blocks: previous.blocks.map((candidate) =>
                  candidate.id === block.id ? block : candidate,
                ),
              }))
            }
            onDelete={() => {
              deleteBlock(selected.id);
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

function nextId(kind: string, blocks: PipelineBlock[]): string {
  for (let index = 1; ; index += 1) {
    const candidate = index === 1 ? kind : `${kind}-${index}`;
    if (!blocks.some((block) => block.id === candidate)) return candidate;
  }
}
