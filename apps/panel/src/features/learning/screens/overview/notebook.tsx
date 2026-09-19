import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Copy, ExternalLink, Play, RefreshCw, Save } from 'lucide-react';
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
import {
  loopback,
  normalizeBase,
  notebookUrl,
  reachable,
  WORKSHOP_BASE,
  WORKSHOP_COMMAND,
} from '@/shared/lib/marimo';
import { Badge, Button, IdChip, Refusal, Spinner } from '@/shared/components/ui/primitives';
import { short } from '@/shared/lib/iam';

/**
 * The notebook a lab hands out, and the copy the participant works in.
 *
 * **The lab names a file and pins its digest; it never carries the source**
 * (ADR_0034), so everything here is read from the notebook runtime by that
 * pin. Three consequences are on the screen rather than hidden.
 *
 * The runtime **may not be there** — no authentication, no sandbox, bound to
 * localhost, and a deployment is free not to run one. That is a sentence and
 * the recipe that starts it, the posture the Query tab takes towards Flow.
 *
 * The live app serves the notebook's **head**, because marimo turns the
 * notebook root into apps and the revision history is kept out of it. Past the
 * pin, the two are said to have drifted and the pinned source is shown beside
 * it rather than one being served quietly as the other.
 *
 * And **nothing runs because somebody opened a lab**: the live app is a click
 * of its own, for the reason `EditorHost::open` stages and stops.
 */
/**
 * The notebook a lab hands out.
 *
 * Which of the two shapes it is, is decided by a field rather than a flag. A
 * **pinned revision** means the bytes are this instance's, kept in the notebook
 * runtime, so there is a source to show, a head that can drift from it and a
 * copy to take. **No revision** means the notebook is a file in a workshop the
 * participant checked out — this instance has never seen it — so what there is
 * to offer is the two places a marimo can be, and the command that starts one.
 */
export function Notebook({
  pinned,
  mine,
  onMine,
}: {
  pinned: LabNotebook | null | undefined;
  /** The participant's own copy of a kept notebook, from the URL. */
  mine: string | undefined;
  onMine: (notebook: string | undefined) => void;
}) {
  if (!pinned) return null;
  return pinned.revision ? (
    <KeptNotebook pinned={pinned} revision={pinned.revision} mine={mine} onMine={onMine} />
  ) : (
    <WorkshopNotebook pinned={pinned} />
  );
}

function KeptNotebook({
  pinned,
  revision,
  mine,
  onMine,
}: {
  pinned: LabNotebook;
  revision: string;
  mine: string | undefined;
  onMine: (notebook: string | undefined) => void;
}) {
  const queryClient = useQueryClient();
  const source = useQuery({
    queryKey: ['ml-pipeline', 'notebook', pinned.path, revision],
    retry: false,
    queryFn: () => getNotebookRevision(pinned.path, revision),
  });
  const head = useQuery({
    queryKey: ['ml-pipeline', 'notebook', pinned.path, 'head'],
    retry: false,
    queryFn: () => getNotebook(pinned.path),
  });

  const copy = useMutation({
    mutationFn: () => createNotebook(source.data?.source),
    onSuccess: (created) => {
      queryClient.setQueryData(['ml-pipeline', 'notebook', created.name, 'head'], created);
      onMine(created.name);
    },
  });

  const drifted = Boolean(head.data && head.data.revision !== revision);
  const missing = source.isError && !(source.error instanceof MlPipelineUnavailableError);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">Notebook</h3>
        <IdChip label="file" value={pinned.path} />
        <IdChip label="pinned" value={short(revision)} full={revision} />
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
              key={head.data?.revision ?? revision}
              src={head.data?.app_url ?? source.data.app_url}
              title={`${pinned.path} — live`}
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

/**
 * A notebook in a workshop somebody checked out, and the two places a marimo
 * for it can be.
 *
 * **Neither of them is this page running anybody's code.** One is a marimo the
 * deployment serves at `/lab-marimo` on this origin — configuration, never an
 * address out of the lab. The other is one the participant starts with the
 * lab's own command, reached on their own loopback; the same box takes the
 * address of an instructor's read-only `marimo run`, because that is the same
 * answer with a different host, and it is typed by the person looking at it.
 *
 * What is remembered is which of the two and what was typed — on this device,
 * per lab, because it is a fact about this machine and not about the lab. A
 * link that carried it would be telling a classmate which port to look at.
 */
function WorkshopNotebook({ pinned }: { pinned: LabNotebook }) {
  const fallbackPort = pinned.port ?? 2718;
  const [where, setWhere] = useRemembered<'served' | 'mine'>(`${pinned.path}/where`, 'served');
  const [typed, setTyped] = useRemembered(`${pinned.path}/address`, loopback(fallbackPort));
  const base = where === 'served' ? WORKSHOP_BASE : normalizeBase(typed);

  const reach = useQuery({
    queryKey: ['lab-marimo', base ?? ''],
    enabled: Boolean(base),
    retry: false,
    refetchOnWindowFocus: true,
    queryFn: () => reachable(base as string),
  });

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">Notebook</h3>
        <IdChip label="file" value={pinned.path} />
      </div>
      <p className="max-w-3xl text-xs text-muted-foreground">
        This lab is worked in marimo. It runs in one of two places and neither of them is this page:
        a marimo this deployment serves, or one you start yourself.
      </p>

      <div role="radiogroup" aria-label="Where marimo runs" className="flex flex-wrap gap-2">
        {(
          [
            ['served', 'Served here'],
            ['mine', 'On my machine'],
          ] as const
        ).map(([value, label]) => (
          <Button
            key={value}
            type="button"
            role="radio"
            aria-checked={where === value}
            size="sm"
            variant={where === value ? 'default' : 'outline'}
            onClick={() => setWhere(value)}
          >
            {label}
          </Button>
        ))}
      </div>

      {where === 'mine' ? (
        <div className="flex flex-col gap-2">
          {pinned.command ? (
            <div className="flex flex-col gap-1">
              <span className="text-xs font-medium">Start it</span>
              <div className="flex flex-wrap items-center gap-2">
                <code className="id rounded-md border border-border px-2 py-1 text-xs">
                  {pinned.command}
                </code>
                <Button
                  size="sm"
                  variant="ghost"
                  type="button"
                  onClick={() => void navigator.clipboard?.writeText(pinned.command as string)}
                >
                  <Copy className="h-3.5 w-3.5" /> Copy
                </Button>
              </div>
            </div>
          ) : (
            <p className="max-w-3xl text-xs text-muted-foreground">
              This lab names no command, so whoever wrote it expects you to know how your workshop
              opens {pinned.path}.
            </p>
          )}
          <label className="flex max-w-xl flex-col gap-1">
            <span className="text-xs font-medium">Address</span>
            <input
              className="h-9 w-full rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              placeholder={loopback(fallbackPort)}
              spellCheck={false}
            />
            <span className="text-xs text-muted-foreground">
              Your own machine by default. An instructor presenting a read-only{' '}
              <span className="font-mono">marimo run --no-token</span> shares their address, and it
              goes here — this is kept on this device and sent nowhere.
            </span>
          </label>
          {typed.trim() !== '' && !base ? (
            <p className="text-xs text-warning">
              That is not an http or https address this page can open.
            </p>
          ) : null}
        </div>
      ) : null}

      {reach.isPending && base ? <Spinner /> : null}

      {base && reach.data === false ? (
        <p className="max-w-3xl text-xs text-muted-foreground">
          {where === 'served' ? (
            <>
              Nothing answers at <span className="font-mono">{WORKSHOP_BASE}</span> on this
              deployment. Whoever runs it points that path at{' '}
              <span className="font-mono">{WORKSHOP_COMMAND}</span> — until then, open the notebook
              on your machine.
            </>
          ) : (
            <>
              Nothing answers at <span className="font-mono">{base}</span> yet. Run the command
              above and this picks it up.
            </>
          )}
        </p>
      ) : null}

      {base && reach.data === true ? (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <a
              href={notebookUrl(base, pinned.path)}
              target="_blank"
              rel="noreferrer"
              className="flex items-center gap-1 text-xs text-primary hover:underline"
            >
              <ExternalLink className="h-3.5 w-3.5" /> open in its own tab
            </a>
            <Button
              size="sm"
              variant="ghost"
              type="button"
              onClick={() => void reach.refetch()}
              disabled={reach.isFetching}
            >
              <RefreshCw className="h-3.5 w-3.5" /> Check again
            </Button>
          </div>
          <details className="max-w-full">
            <summary className="flex cursor-pointer items-center gap-1 text-xs text-primary">
              <Play className="h-3.5 w-3.5" /> Open it here
            </summary>
            <iframe
              key={base}
              src={notebookUrl(base, pinned.path)}
              title={`${pinned.path} — marimo`}
              className="mt-2 h-[40rem] w-full rounded-md border border-border bg-background"
              loading="lazy"
            />
          </details>
        </>
      ) : null}
    </div>
  );
}

/**
 * A choice this device keeps.
 *
 * Wrapped in try/catch on both sides because a private window, blocked site
 * data or a thumbnail capture can make either throw, and a lab that would not
 * render because a preference could not be read would be the wrong failure.
 */
function useRemembered<T extends string>(key: string, fallback: T): [T, (next: T) => void] {
  const full = `aiwatcher.lab-marimo.${key}`;
  const [value, setValue] = React.useState<T>(() => {
    try {
      return (window.localStorage.getItem(full) as T | null) ?? fallback;
    } catch {
      return fallback;
    }
  });
  return [
    value,
    (next: T) => {
      setValue(next);
      try {
        window.localStorage.setItem(full, next);
      } catch {
        // Applied for this session; a device that will not keep it is not an
        // error worth a box on a lesson.
      }
    },
  ];
}
