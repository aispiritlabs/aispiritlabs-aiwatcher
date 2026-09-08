import * as React from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink, RefreshCw, Save, Trash2 } from 'lucide-react';

import type { BlockSpec, PipelineBlock } from '@/api/generated/types.gen';
import { Badge, Button, Card, Spinner } from '@/components/ui/primitives';
import { blockLabel } from '@/components/pipeline-canvas';
import { fetchDatasets, type FlowDataset } from '@/lib/flow';
import { getNotebook, getNotebooks, saveNotebook } from '@/lib/ml-pipeline';
import { readCall } from '@/lib/pipeline';
import { cn } from '@/lib/utils';

/**
 * What opens when a block is clicked: its settings, or its code.
 *
 * Which of the two it is depends on the block, and that difference is the
 * point of having blocks at all. A source is a form, because "which corpus,
 * which split, how many rows" is a form. A Flow PHP block is a text editor,
 * because a transformation is text. A notebook is *both* — its code, and the
 * live app that code produces, running against the rows the chain last put in
 * front of it.
 */

const INPUT =
  'h-9 w-full rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary';

export function BlockInspector({
  block,
  onChange,
  onDelete,
  children,
}: {
  block: PipelineBlock;
  onChange: (block: PipelineBlock) => void;
  onDelete: () => void;
  /** The kind-specific action the page owns, e.g. publishing a view. */
  children?: React.ReactNode;
}) {
  const setSpec = (spec: BlockSpec) => onChange({ ...block, spec });

  return (
    <Card className="flex min-w-0 flex-col">
      <div className="flex items-center gap-2 border-b border-border p-3">
        <Badge>{blockLabel(block.spec.kind)}</Badge>
        <input
          value={block.title}
          onChange={(event) => onChange({ ...block, title: event.target.value })}
          placeholder="Untitled block"
          className="h-8 min-w-0 flex-1 rounded-md border border-transparent bg-transparent px-2 text-sm font-medium outline-none hover:border-border focus-visible:border-border"
        />
        <Button variant="ghost" size="sm" onClick={onDelete} title="Remove this block">
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>

      <div className="flex flex-col gap-3 p-3">
        {block.spec.kind === 'source' ? (
          <SourceSettings spec={block.spec} onChange={setSpec} />
        ) : null}
        {block.spec.kind === 'transform' ? (
          <TransformCode spec={block.spec} onChange={setSpec} />
        ) : null}
        {block.spec.kind === 'notebook' ? (
          <NotebookEditor spec={block.spec} onChange={setSpec} />
        ) : null}
        {block.spec.kind === 'view' ? <ViewSettings spec={block.spec} onChange={setSpec} /> : null}
        {children}
      </div>
    </Card>
  );
}

/**
 * The read: a dataset from the Flow catalog, and the arguments it declares.
 *
 * The arguments are rendered from the catalog rather than typed free-hand,
 * because the catalog is where "which arguments does this dataset take" is
 * answered — the aiwatcher API rejects an unknown query parameter, so a
 * hand-typed one is somebody else's 400 with nothing pointing at the word that
 * caused it.
 */
function SourceSettings({
  spec,
  onChange,
}: {
  spec: Extract<BlockSpec, { kind: 'source' }>;
  onChange: (spec: BlockSpec) => void;
}) {
  const catalog = useQuery({ queryKey: ['flow', 'datasets'], queryFn: fetchDatasets });
  const dataset: FlowDataset | undefined = catalog.data?.datasets.find(
    (candidate) => candidate.name === spec.dataset,
  );

  const setArgument = (name: string, value: string) =>
    onChange({ ...spec, arguments: { ...spec.arguments, [name]: value } });

  return (
    <>
      <Field label="Dataset" hint={dataset?.grain}>
        <select
          value={spec.dataset}
          onChange={(event) => onChange({ ...spec, dataset: event.target.value, arguments: {} })}
          className={INPUT}
        >
          {catalog.data?.datasets.map((candidate) => (
            <option key={candidate.name} value={candidate.name}>
              {candidate.name}
            </option>
          )) ?? <option value={spec.dataset}>{spec.dataset}</option>}
        </select>
      </Field>
      {dataset ? <p className="text-xs text-muted-foreground">{dataset.description}</p> : null}
      {(dataset?.parameters ?? []).map((parameter) => (
        <Field
          key={parameter.name}
          label={`${parameter.name}${parameter.required ? ' *' : ''}`}
          hint={parameter.description}
        >
          {parameter.values.length > 0 ? (
            <select
              value={spec.arguments?.[parameter.name] ?? ''}
              onChange={(event) => setArgument(parameter.name, event.target.value)}
              className={INPUT}
            >
              <option value="">—</option>
              {parameter.values.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          ) : (
            <input
              value={spec.arguments?.[parameter.name] ?? ''}
              onChange={(event) => setArgument(parameter.name, event.target.value)}
              className={INPUT}
            />
          )}
        </Field>
      ))}
      {catalog.isError ? (
        <p className="text-xs text-warning">
          The Flow service is not answering, so this is the dataset name as saved. Start it with{' '}
          <code className="id">just flow-serve</code>.
        </p>
      ) : null}
      <code className="id block overflow-x-auto rounded-md bg-muted/40 p-2 text-[11px] text-primary">
        {readCall(spec)}
      </code>
    </>
  );
}

/** The Flow PHP steps this block adds to the read. */
function TransformCode({
  spec,
  onChange,
}: {
  spec: Extract<BlockSpec, { kind: 'transform' }>;
  onChange: (spec: BlockSpec) => void;
}) {
  return (
    <>
      <p className="text-xs text-muted-foreground">
        The tail of the pipeline, one step per line. No <code className="id">data_frame()</code>, no{' '}
        <code className="id">read()</code> and no <code className="id">write()</code> — the chain
        contributes those, and every Flow block before the first notebook runs as one query.
      </p>
      <textarea
        value={spec.steps ?? ''}
        onChange={(event) => onChange({ ...spec, steps: event.target.value })}
        spellCheck={false}
        rows={10}
        className="id w-full resize-y rounded-md border border-border bg-transparent p-2 outline-none focus-visible:ring-2 focus-visible:ring-primary"
      />
    </>
  );
}

/**
 * A notebook: which one, what it starts with, its code, and the live app.
 *
 * The source is read from and written to the notebook runtime rather than held
 * in the block, because that file is what marimo serves, what a run imports and
 * what a test reads. The block *pins* the revision it was saved against, and
 * this says so when the two have drifted — which is the honest state, not an
 * error: somebody edited the notebook, and this pipeline was saved before that.
 */
function NotebookEditor({
  spec,
  onChange,
}: {
  spec: Extract<BlockSpec, { kind: 'notebook' }>;
  onChange: (spec: BlockSpec) => void;
}) {
  const queryClient = useQueryClient();
  const notebooks = useQuery({ queryKey: ['ml-pipeline', 'notebooks'], queryFn: getNotebooks });
  const notebook = useQuery({
    queryKey: ['ml-pipeline', 'notebook', spec.notebook],
    queryFn: () => getNotebook(spec.notebook),
    retry: false,
  });

  const [draft, setDraft] = React.useState<string | null>(null);
  const [params, setParams] = React.useState(() => JSON.stringify(spec.params ?? {}, null, 2));
  const [paramsError, setParamsError] = React.useState<string | null>(null);

  // The editor follows the selection: a different notebook is a different file,
  // and carrying an unsaved draft across would be offering to overwrite one
  // file with another's contents.
  React.useEffect(() => setDraft(null), [spec.notebook]);
  React.useEffect(() => {
    setParams(JSON.stringify(spec.params ?? {}, null, 2));
    setParamsError(null);
  }, [spec.notebook]);

  const save = useMutation({
    mutationFn: async () => saveNotebook(spec.notebook, draft ?? ''),
    onSuccess: (saved) => {
      setDraft(null);
      onChange({ ...spec, revision: saved.revision });
      void queryClient.invalidateQueries({ queryKey: ['ml-pipeline'] });
    },
  });

  const source = draft ?? notebook.data?.source ?? '';
  const drifted =
    spec.revision && notebook.data?.revision && spec.revision !== notebook.data.revision;

  return (
    <>
      <Field label="Notebook">
        <select
          value={spec.notebook}
          onChange={(event) =>
            onChange({ ...spec, notebook: event.target.value, revision: undefined })
          }
          className={INPUT}
        >
          {notebooks.data?.map((candidate) => (
            <option key={candidate.name} value={candidate.name}>
              {candidate.name} — {candidate.title}
            </option>
          )) ?? <option value={spec.notebook}>{spec.notebook}</option>}
        </select>
      </Field>

      {notebook.isError ? (
        <p className="text-xs text-warning">
          The notebook runtime is not answering. Start it with{' '}
          <code className="id">just ml-pipeline-serve</code>; the rest of the chain still runs.
        </p>
      ) : null}

      {drifted ? (
        <p className="rounded-md border border-warning/40 bg-warning/5 p-2 text-xs">
          This pipeline was saved against revision{' '}
          <code className="id">{spec.revision?.slice(0, 12)}</code> and the notebook is now{' '}
          <code className="id">{notebook.data?.revision.slice(0, 12)}</code>. A managed run still
          executes the revision it pinned — the runtime keeps every source it has been given — so
          this is what runs next rather than something that is broken. Saving the pipeline pins the
          new one.
        </p>
      ) : null}

      <Field
        label="Settings"
        hint="What the notebook's mo.ui elements start at. A headless run has these instead of somebody moving a slider."
      >
        <textarea
          value={params}
          onChange={(event) => {
            setParams(event.target.value);
            try {
              const parsed: unknown = JSON.parse(event.target.value || '{}');
              if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
                throw new Error('the settings are a JSON object');
              }
              setParamsError(null);
              onChange({ ...spec, params: parsed as Record<string, unknown> });
            } catch (error) {
              setParamsError(error instanceof Error ? error.message : 'not JSON');
            }
          }}
          spellCheck={false}
          rows={4}
          className={cn(
            'id w-full resize-y rounded-md border bg-transparent p-2 outline-none focus-visible:ring-2 focus-visible:ring-primary',
            paramsError ? 'border-danger' : 'border-border',
          )}
        />
      </Field>
      {paramsError ? <p className="text-xs text-danger">{paramsError}</p> : null}

      <Field label="Code">
        <textarea
          value={source}
          onChange={(event) => setDraft(event.target.value)}
          spellCheck={false}
          rows={14}
          disabled={notebook.isLoading || notebook.isError}
          className="id w-full resize-y rounded-md border border-border bg-transparent p-2 outline-none focus-visible:ring-2 focus-visible:ring-primary disabled:opacity-50"
        />
      </Field>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={() => save.mutate()} disabled={draft === null || save.isPending}>
          {save.isPending ? <Spinner /> : <Save className="h-3.5 w-3.5" />} Save notebook
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => void notebook.refetch()}
          disabled={notebook.isFetching}
        >
          <RefreshCw className="h-3.5 w-3.5" /> Reload
        </Button>
        {notebook.data ? (
          <a
            href={notebook.data.app_url}
            target="_blank"
            rel="noreferrer"
            className="flex items-center gap-1 text-xs text-primary hover:underline"
          >
            <ExternalLink className="h-3.5 w-3.5" /> open in its own tab
          </a>
        ) : null}
      </div>
      {save.error ? <p className="text-xs text-danger">{save.error.message}</p> : null}

      {notebook.data ? (
        <div className="flex flex-col gap-1">
          <p className="text-xs text-muted-foreground">
            Live, over the rows this block was last given. Move a control here and the notebook
            re-runs; the settings above are what a headless run starts from.
          </p>
          {/* Keyed by revision so a save reloads the frame: marimo has already
              executed the old code, and nothing in the iframe knows the file
              underneath it changed. */}
          <iframe
            key={notebook.data.revision}
            src={notebook.data.app_url}
            title={`${spec.notebook} — live`}
            className="h-[30rem] w-full rounded-md border border-border bg-background"
          />
        </div>
      ) : null}
    </>
  );
}

/** The end of the chain: what the rows are published as. */
function ViewSettings({
  spec,
  onChange,
}: {
  spec: Extract<BlockSpec, { kind: 'view' }>;
  onChange: (spec: BlockSpec) => void;
}) {
  return (
    <Field
      label="Publish as"
      hint="An immutable dataset version, named by the exact rows in it. Leave it empty to look without saving."
    >
      <input
        value={spec.dataset ?? ''}
        onChange={(event) => onChange({ ...spec, dataset: event.target.value || undefined })}
        placeholder="curation/pii-masked-sample"
        className={INPUT}
      />
    </Field>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
      {hint ? <span className="text-[11px] text-muted-foreground">{hint}</span> : null}
    </label>
  );
}
