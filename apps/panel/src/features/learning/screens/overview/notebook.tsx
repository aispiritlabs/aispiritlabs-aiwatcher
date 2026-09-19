import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink, Play, RefreshCw, Save } from 'lucide-react';
import * as React from 'react';

import type { LabNotebook } from '@/api/generated';
import {
  createNotebook,
  getNotebook,
  getNotebookRevision,
  MlPipelineUnavailableError,
  saveNotebook,
  type NotebookSource,
} from '@/shared/lib/ml-pipeline';
import { Badge, Button, IdChip, Refusal, Spinner } from '@/shared/components/ui/primitives';
import { short } from '@/shared/lib/iam';

/**
 * The notebook a lab hands out, and the copy the participant works in.
 *
 * **The lab names a file and pins its digest; it never carries the source.**
 * That file is what marimo serves, what `ml_pipeline.step` imports and what
 * the participant edits — a copy of it in the registry would be a second
 * source of truth for something that has to stay runnable on its own, which is
 * the rule `services/ml_pipeline/CLAUDE.md` already states for a curation
 * block. So everything here is read from the notebook runtime, by the pin the
 * lab published.
 *
 * Three consequences are visible on the screen rather than hidden.
 *
 * The runtime **may not be there**. It has no authentication and no sandbox,
 * it binds to localhost, and a deployment is free not to run one; that is a
 * sentence and the name of the recipe that starts it, not an error box — the
 * same posture the Query tab takes towards Flow.
 *
 * The live app serves the notebook's **head**, because marimo turns the
 * notebook root into apps and the revision history is kept out of it. When the
 * head has moved past what this lab pinned, the two are said to have drifted
 * and the pinned source is shown beside it, rather than quietly running
 * something the lab did not hand out.
 *
 * And **nothing runs because somebody opened a lab.** Opening the live app is
 * a click of its own, for the reason `EditorHost::open` stages and stops:
 * executing on open runs somebody's code because a page was looked at.
 */
export function Notebook({
  pinned,
  mine,
  onMine,
}: {
  pinned: LabNotebook | null | undefined;
  /** The participant's own copy, from the URL. Theirs, and not this lab's. */
  mine: string | undefined;
  onMine: (notebook: string | undefined) => void;
}) {
  const queryClient = useQueryClient();
  const source = useQuery({
    queryKey: ['ml-pipeline', 'notebook', pinned?.name, pinned?.revision],
    enabled: Boolean(pinned),
    retry: false,
    queryFn: () => getNotebookRevision(pinned!.name, pinned!.revision),
  });
  const head = useQuery({
    queryKey: ['ml-pipeline', 'notebook', pinned?.name, 'head'],
    enabled: Boolean(pinned),
    retry: false,
    queryFn: () => getNotebook(pinned!.name),
  });

  const copy = useMutation({
    mutationFn: () => createNotebook(source.data?.source),
    onSuccess: (created) => {
      queryClient.setQueryData(['ml-pipeline', 'notebook', created.name, 'head'], created);
      onMine(created.name);
    },
  });

  if (!pinned) return null;

  const drifted = Boolean(head.data && head.data.revision !== pinned.revision);
  const missing = source.isError && !(source.error instanceof MlPipelineUnavailableError);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">Notebook</h3>
        <IdChip label="file" value={pinned.name} />
        <IdChip label="pinned" value={short(pinned.revision)} full={pinned.revision} />
        {drifted ? <Badge tone="warning">the file has been edited since</Badge> : null}
      </div>

      {source.isPending ? <Spinner /> : null}

      {source.error instanceof MlPipelineUnavailableError ? (
        <p className="max-w-3xl text-xs text-muted-foreground">
          The notebook runtime is not running here, so this lab&apos;s notebook cannot be opened.
          Start it with <span className="font-mono">just ml-pipeline-serve</span>; everything else
          on this lab reads without it.
        </p>
      ) : null}

      {missing ? (
        <Refusal
          error={source.error}
          fallback="this runtime does not hold the source this lab pins"
        />
      ) : null}

      {source.data ? (
        <>
          {drifted ? (
            <p className="max-w-3xl text-xs text-warning">
              The live app below runs the notebook&apos;s current head (
              <span className="font-mono">{short(head.data!.revision)}</span>), which is not what
              this lab pinned. The source this lab hands out is the one shown here, and a copy
              starts from it.
            </p>
          ) : null}

          <details className="max-w-3xl">
            <summary className="cursor-pointer text-xs text-primary">
              Read the notebook this lab hands out
            </summary>
            <pre className="id mt-2 max-h-96 overflow-auto rounded-md border border-border p-2 text-xs">
              {source.data.source}
            </pre>
          </details>

          <details className="max-w-3xl">
            <summary className="cursor-pointer text-xs text-primary">
              Open it and run it — the live notebook, with its own widgets
            </summary>
            <iframe
              key={head.data?.revision ?? pinned.revision}
              src={head.data?.app_url ?? source.data.app_url}
              title={`${pinned.name} — live`}
              className="mt-2 h-[30rem] w-full rounded-md border border-border bg-background"
              loading="lazy"
            />
          </details>

          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" onClick={() => copy.mutate()} disabled={copy.isPending || !!mine}>
              {copy.isPending ? <Spinner /> : null} Work in a copy of my own
            </Button>
            <a
              href={source.data.app_url}
              target="_blank"
              rel="noreferrer"
              className="flex items-center gap-1 text-xs text-primary hover:underline"
            >
              <ExternalLink className="h-3.5 w-3.5" /> open in its own tab
            </a>
          </div>
          {copy.error ? <Refusal error={copy.error} fallback="a copy could not be made" /> : null}
        </>
      ) : null}

      {mine ? <Mine name={mine} onRelease={() => onMine(undefined)} /> : null}
    </div>
  );
}

/**
 * The participant's own file: editable, runnable, and theirs.
 *
 * A copy rather than the lab's own notebook because the runtime holds one
 * namespace: thirty people saving `lab_03_agent` would be thirty people
 * overwriting each other, and the last save would be what the next reader of
 * the lab was handed. `createNotebook` mints a name nobody else has, which is
 * the same reason the curation block library mints one.
 *
 * **Nothing here hands the work in.** A submission is a staged recording or
 * answers a worker generated, scored against the lab's card by a run somebody
 * starts, and `POST /evaluation-runs/{id}/start` has no project twin yet
 * (ADR_0033 opens none before the data plane follows). So this says what it is
 * — your working copy — instead of a button that would be refused.
 */
function Mine({ name, onRelease }: { name: string; onRelease: () => void }) {
  const queryClient = useQueryClient();
  const notebook = useQuery({
    queryKey: ['ml-pipeline', 'notebook', name, 'head'],
    retry: false,
    queryFn: () => getNotebook(name),
  });
  const [draft, setDraft] = React.useState<string | null>(null);
  // A different copy is a different file, and carrying an unsaved draft across
  // would be offering to overwrite one with another's contents.
  React.useEffect(() => setDraft(null), [name]);

  const source = draft ?? notebook.data?.source ?? '';
  const save = useMutation({
    mutationFn: (): Promise<NotebookSource> => saveNotebook(name, source),
    onSuccess: (saved) => {
      queryClient.setQueryData(['ml-pipeline', 'notebook', name, 'head'], saved);
      setDraft(null);
    },
  });

  return (
    <div className="flex flex-col gap-2 rounded-md border border-dashed border-border p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="text-xs font-medium">My copy</h4>
        <IdChip label="file" value={name} />
        {notebook.data ? (
          <IdChip
            label="saved"
            value={short(notebook.data.revision)}
            full={notebook.data.revision}
          />
        ) : null}
        <Button size="sm" variant="ghost" className="ml-auto" onClick={onRelease}>
          Put it away
        </Button>
      </div>

      {notebook.isPending ? <Spinner /> : null}
      {notebook.isError ? (
        <Refusal error={notebook.error} fallback="your copy could not be read" />
      ) : null}

      {notebook.data ? (
        <>
          <textarea
            value={source}
            onChange={(event) =>
              setDraft(event.target.value === notebook.data.source ? null : event.target.value)
            }
            spellCheck={false}
            rows={14}
            aria-label="My notebook"
            className="id w-full resize-y rounded-md border border-border bg-transparent p-2 outline-none focus-visible:ring-2 focus-visible:ring-primary"
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              onClick={() => save.mutate()}
              disabled={draft === null || save.isPending}
            >
              {save.isPending ? <Spinner /> : <Save className="h-3.5 w-3.5" />} Save
            </Button>
            {draft !== null ? (
              <Button size="sm" variant="ghost" onClick={() => setDraft(null)}>
                Discard my edits
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="outline"
              onClick={() => void notebook.refetch()}
              disabled={notebook.isFetching || draft !== null}
            >
              <RefreshCw className="h-3.5 w-3.5" /> Reload
            </Button>
          </div>
          {draft !== null ? (
            <p role="status" className="text-xs text-warning">
              Unsaved changes. The live app runs what is saved.
            </p>
          ) : null}
          {save.error ? <Refusal error={save.error} fallback="your copy was not saved" /> : null}

          <details>
            <summary className="flex cursor-pointer items-center gap-1 text-xs text-primary">
              <Play className="h-3.5 w-3.5" /> Run my copy
            </summary>
            <iframe
              key={notebook.data.revision}
              src={notebook.data.app_url}
              title={`${name} — live`}
              className="mt-2 h-[30rem] w-full rounded-md border border-border bg-background"
              loading="lazy"
            />
          </details>
          <p className="text-xs text-muted-foreground">
            This is your working copy in the notebook runtime. Handing it in is a separate step and
            this panel does not have one yet: a submission is scored by a run, and the route that
            starts one is not open to a project.
          </p>
        </>
      ) : null}
    </div>
  );
}
