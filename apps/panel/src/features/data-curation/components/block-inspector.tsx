import * as React from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink, RefreshCw, Save, Trash2 } from 'lucide-react';

import type { BlockSpec, PipelineBlock } from '@/api/generated/types.gen';
import { Badge, Button, Card, Spinner } from '@/shared/components/ui/primitives';
import { blockLabel } from '@/features/data-curation/components/pipeline-canvas';
import { contentFor } from '@/shared/lib/engine-content';
import {
  ENGINE_LABEL,
  fetchDatasets,
  useQueryEngine,
  writtenElsewhere,
  type QueryDataset,
} from '@/shared/lib/query';
import {
  createNotebook,
  getNotebook,
  getNotebookRevision,
  getNotebooks,
  saveNotebook,
} from '@/shared/lib/ml-pipeline';
import { pythonRead, readCall } from '@/features/data-curation/lib/pipeline';
import { cn } from '@/shared/lib/utils';

/**
 * What opens when a block is clicked: its settings, or its code.
 *
 * Which of the two it is depends on the block, and that difference is the
 * point of having blocks at all. A source is a form, because "which corpus,
 * which split, how many rows" is a form. A query block is a text editor,
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
  disabled = false,
  onDirtyChange,
}: {
  block: PipelineBlock;
  onChange: (block: PipelineBlock) => void;
  onDelete: () => void;
  /** The kind-specific action the page owns, e.g. publishing a view. */
  children?: React.ReactNode;
  disabled?: boolean;
  onDirtyChange?: (id: string, dirty: boolean) => void;
}) {
  const setSpec = (spec: BlockSpec) => onChange({ ...block, spec });

  return (
    <fieldset disabled={disabled} className="min-w-0">
      <Card className="flex h-fit min-w-0 flex-col">
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
            <NotebookEditor
              key={block.id}
              blockId={block.id}
              spec={block.spec}
              onChange={setSpec}
              onDirtyChange={onDirtyChange}
            />
          ) : null}
          {block.spec.kind === 'approval' ? (
            <ApprovalSettings spec={block.spec} onChange={setSpec} />
          ) : null}
          {block.spec.kind === 'view' ? (
            <ViewSettings spec={block.spec} onChange={setSpec} />
          ) : null}
          {children}
        </div>
      </Card>
    </fieldset>
  );
}

/**
 * The read: a dataset from the query engine's catalog, and the arguments it declares.
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
  const deployed = useQueryEngine().data?.engine;
  const dataset: QueryDataset | undefined = catalog.data?.datasets.find(
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
          The query engine is not answering, so this is the dataset name as saved. Start it with{' '}
          <code className="id">just query-serve</code>.
        </p>
      ) : null}
      <code className="id block overflow-x-auto rounded-md bg-muted/40 p-2 text-[11px] text-primary">
        {deployed && deployed !== 'flow' ? readInPython(spec) : readCall(spec)}
      </code>
    </>
  );
}

/** The read in Python, or the sentence a Python engine's compiler would refuse it with. */
function readInPython(spec: BlockSpec): string {
  try {
    return pythonRead(spec);
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

/**
 * The steps this block adds to the read, in the language of the engine it was
 * written for. Written for another than the one deployed, it is shown and not
 * edited: the text is another language, and saving an edit of it here would be
 * a guess at a language this deployment cannot even check (AW-3).
 */
function TransformCode({
  spec,
  onChange,
}: {
  spec: Extract<BlockSpec, { kind: 'transform' }>;
  onChange: (spec: BlockSpec) => void;
}) {
  const deployed = useQueryEngine().data?.engine;
  const written = spec.engine ?? 'flow';
  const foreign = writtenElsewhere(written, deployed);
  return (
    <>
      <p className="text-xs text-muted-foreground">
        {contentFor(written)?.transformHelp ??
          `${ENGINE_LABEL[written]} over the rows the blocks before this one produced.`}
      </p>
      {foreign ? <p className="text-xs text-warning">{foreign}</p> : null}
      <textarea
        aria-label={`${ENGINE_LABEL[written]} transformation code`}
        readOnly={Boolean(foreign)}
        value={spec.steps ?? ''}
        onChange={(event) => onChange({ ...spec, steps: event.target.value })}
        spellCheck={false}
        rows={20}
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
 * the editor reads that exact revision. A copy gets its own file and can evolve
 * independently of the original.
 */
function NotebookEditor({
  spec,
  onChange,
  blockId,
  onDirtyChange,
}: {
  spec: Extract<BlockSpec, { kind: 'notebook' }>;
  onChange: (spec: BlockSpec) => void;
  blockId: string;
  onDirtyChange?: (id: string, dirty: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const notebooks = useQuery({ queryKey: ['ml-pipeline', 'notebooks'], queryFn: getNotebooks });
  const notebook = useQuery({
    queryKey: ['ml-pipeline', 'notebook', spec.notebook, spec.revision ?? 'head'],
    queryFn: () =>
      spec.revision
        ? getNotebookRevision(spec.notebook, spec.revision)
        : getNotebook(spec.notebook),
    retry: false,
  });

  const [draft, setDraft] = React.useState<string | null>(null);
  const [params, setParams] = React.useState(() => JSON.stringify(spec.params ?? {}, null, 2));
  const [paramsError, setParamsError] = React.useState<string | null>(null);

  const dirty = draft !== null || paramsError !== null;

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
  const copy = useMutation({
    mutationFn: (empty: boolean) => (empty ? createNotebook() : createNotebook(source)),
    onSuccess: (created) => {
      setDraft(null);
      onChange({ ...spec, notebook: created.name, revision: created.revision });
      void queryClient.invalidateQueries({ queryKey: ['ml-pipeline'] });
    },
  });
  React.useEffect(() => {
    onDirtyChange?.(blockId, dirty || copy.isPending || save.isPending);
    return () => onDirtyChange?.(blockId, false);
  }, [blockId, dirty, copy.isPending, save.isPending, onDirtyChange]);

  return (
    <>
      <Field label="Notebook">
        <select
          disabled={dirty || save.isPending || copy.isPending}
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

      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={dirty || copy.isPending}
          onClick={() => copy.mutate(true)}
        >
          New notebook
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!source || copy.isPending || save.isPending}
          onClick={() => copy.mutate(false)}
        >
          Save as copy
        </Button>
      </div>
      {copy.error ? <p className="text-xs text-danger">{copy.error.message}</p> : null}

      {notebook.isError ? <p className="text-xs text-warning">{notebook.error.message}</p> : null}

      {spec.revision ? (
        <p className="text-xs text-muted-foreground">
          Editing pinned revision {spec.revision.slice(0, 12)}. Save notebook updates this block;
          Save as copy creates an independent file.
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
        {draft !== null ? (
          <Button size="sm" variant="ghost" onClick={() => setDraft(null)}>
            Discard code edits
          </Button>
        ) : null}
        <Button
          variant="outline"
          size="sm"
          onClick={() => void notebook.refetch()}
          disabled={notebook.isFetching || dirty}
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
          <details>
            <summary className="cursor-pointer text-xs text-primary">
              Open live output (current notebook head)
            </summary>
            <iframe
              key={notebook.data.revision}
              src={notebook.data.app_url}
              title={`${spec.notebook} — live`}
              className="h-[30rem] w-full rounded-md border border-border bg-background"
              loading="lazy"
            />
          </details>
        </div>
      ) : null}
    </>
  );
}

/**
 * A gate: what is asked, and the answers offered.
 *
 * The answers are a list rather than free text because the server matches an
 * answer against them by equality and refuses anything else — so offering a
 * text box for a question that declared choices would be offering an answer
 * that is going to come back a 409. Leave the list empty and the answer *is*
 * free text, which is the same rule read the other way.
 *
 * Who may answer *is* a control, and only ever raises the floor. The answer
 * route reads the question's own role and requires it; every write here already
 * needs an editor, so those are the two a gate may name and the registry
 * refuses anything weaker — a gate promising that a viewer answers it would be
 * offering buttons to somebody about to be refused.
 */
function ApprovalSettings({
  spec,
  onChange,
}: {
  spec: Extract<BlockSpec, { kind: 'approval' }>;
  onChange: (spec: BlockSpec) => void;
}) {
  return (
    <>
      <Field
        label="Question"
        hint="What somebody reads before the run goes on. The chain stops here until it is answered."
      >
        <textarea
          aria-label="Approval question"
          value={spec.prompt ?? ''}
          onChange={(event) => onChange({ ...spec, prompt: event.target.value })}
          rows={3}
          className="w-full resize-y rounded-md border border-border bg-transparent p-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
      </Field>
      <Field
        label="Answers"
        hint="One per line. Leave it empty for a typed answer; anything listed is the whole set, and nothing else is accepted."
      >
        <textarea
          aria-label="Approval answers"
          value={(spec.choices ?? []).join('\n')}
          onChange={(event) =>
            onChange({
              ...spec,
              choices: event.target.value
                .split('\n')
                .map((choice) => choice.trim())
                .filter(Boolean),
            })
          }
          rows={4}
          className="w-full resize-y rounded-md border border-border bg-transparent p-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
      </Field>
      <Field
        label="Answered by"
        hint="A gate raises the floor and never lowers it: every write here already needs an editor."
      >
        <select
          aria-label="Answered by"
          value={spec.role ?? 'editor'}
          onChange={(event) => onChange({ ...spec, role: event.target.value })}
          className={INPUT}
        >
          <option value="editor">an editor</option>
          <option value="admin">an admin</option>
        </select>
      </Field>
      <Field
        label="Wait for"
        hint="Empty waits as long as it takes, which is the honest thing for a decision somebody has to think about."
      >
        <div className="flex flex-wrap items-center gap-2">
          <input
            aria-label="Wait for"
            type="number"
            min={1}
            value={spec.timeout_seconds ?? ''}
            placeholder="no deadline"
            onChange={(event) => {
              const seconds = Number(event.target.value);
              const timeout = event.target.value === '' || !seconds ? undefined : seconds;
              // The two are authored together and the registry refuses them
              // apart, so dropping the deadline drops the policy with it
              // rather than leaving a rule nothing can reach.
              onChange({
                ...spec,
                timeout_seconds: timeout,
                on_timeout: timeout ? (spec.on_timeout ?? { on: 'fail' }) : { on: 'fail' },
              });
            }}
            className={cn(INPUT, 'w-36')}
          />
          <span className="text-xs text-muted-foreground">seconds</span>
        </div>
      </Field>
      {spec.timeout_seconds ? (
        <Field label="And then" hint="What happens when nobody has answered by the deadline.">
          <select
            aria-label="And then"
            value={spec.on_timeout?.on ?? 'fail'}
            onChange={(event) => {
              const on = event.target.value;
              onChange({
                ...spec,
                on_timeout:
                  on === 'answer'
                    ? { on: 'answer', response: spec.choices?.[0] ?? '' }
                    : { on: on as 'fail' | 'skip' },
              });
            }}
            className={INPUT}
          >
            <option value="fail">stop the run</option>
            <option value="skip">go on without it</option>
            <option value="answer">answer it for them</option>
          </select>
        </Field>
      ) : null}
      {spec.timeout_seconds && spec.on_timeout?.on === 'answer' ? (
        <Field
          label="With"
          hint="An answer this question offers. Recorded as the timeout's, never as a person's."
        >
          {spec.choices?.length ? (
            <select
              aria-label="With"
              value={String(spec.on_timeout.response ?? '')}
              onChange={(event) =>
                onChange({ ...spec, on_timeout: { on: 'answer', response: event.target.value } })
              }
              className={INPUT}
            >
              {spec.choices.map((choice) => (
                <option key={choice} value={choice}>
                  {choice}
                </option>
              ))}
            </select>
          ) : (
            <input
              aria-label="With"
              value={String(spec.on_timeout.response ?? '')}
              onChange={(event) =>
                onChange({ ...spec, on_timeout: { on: 'answer', response: event.target.value } })
              }
              className={INPUT}
            />
          )}
        </Field>
      ) : null}
      <p className="text-xs text-muted-foreground">
        A run waiting here is on its own card, where the question and these answers are what a
        person sees.
      </p>
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
