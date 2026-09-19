import { useQuery } from '@tanstack/react-query';
import { Upload } from 'lucide-react';
import * as React from 'react';

import { projectListScorecards, projectListScorecardVersions } from '@/api/generated';
import type { LabNotebook, LabPublishRequest, LabTests } from '@/api/generated';
import { usePublishLab } from '@/features/learning/lib/labs';
import {
  getNotebooks,
  saveNotebook,
  EMPTY_NOTEBOOK_SOURCE,
  MlPipelineUnavailableError,
} from '@/shared/lib/ml-pipeline';
import { Badge, Button, IdChip, Refusal, Spinner } from '@/shared/components/ui/primitives';
import { short } from '@/shared/lib/iam';
import { answerOf } from '@/shared/lib/result';

/**
 * Writing a lab: the notes somebody reads, the notebook they work in, and the
 * measurement their work is held to.
 *
 * Three fields of the four are just fields. The notebook is the one that is
 * not, and the reason is where its source lives: the lab names a file in the
 * notebook runtime and pins the `sha256` of the source it was written against,
 * so uploading one here means **saving it to the runtime first** and pinning
 * what the runtime answered. A digest computed in the browser would be a
 * second implementation of a content address for a file the browser is not the
 * keeper of, and the day it disagreed a lab would pin a revision nothing holds.
 *
 * An upload never lands on a name the runtime already has. One namespace holds
 * every notebook this instance can run, including the two that ship with it,
 * and a lab that overwrote `pii_detection` would break a curation pipeline from
 * the Learning area.
 *
 * Publishing and *setting* are separate, as they are for a prompt: a publish
 * with no label is next week's lab, written while the class is on this one.
 */
const INPUT =
  'h-9 w-full rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary';

export function ComposeLab({
  organization,
  project,
  onPublished,
}: {
  organization: string;
  project: string;
  onPublished: (name: string) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const [name, setName] = React.useState('');
  const [title, setTitle] = React.useState('');
  const [position, setPosition] = React.useState('');
  const [brief, setBrief] = React.useState('');
  const [notes, setNotes] = React.useState('');
  const [label, setLabel] = React.useState(true);
  const [notebook, setNotebook] = React.useState<LabNotebook | undefined>();
  const [tests, setTests] = React.useState<LabTests | undefined>();
  const publish = usePublishLab(organization, project);

  const body: LabPublishRequest = {
    name,
    title: title.trim(),
    brief,
    ...(position.trim() ? { position: Number(position) } : {}),
    ...(notebook ? { notebook } : {}),
    ...(tests ? { tests } : {}),
    ...(notes.trim() ? { notes: notes.trim() } : {}),
    ...(label ? { label: 'published' } : {}),
  };
  const ready = /^[a-z0-9][a-z0-9._-]{0,63}$/.test(name) && title.trim() !== '' && brief !== '';

  if (!open) {
    return (
      <Button size="sm" variant="outline" className="self-start" onClick={() => setOpen(true)}>
        Write a lab
      </Button>
    );
  }

  return (
    <form
      className="flex flex-col gap-3 rounded-md border border-border p-4"
      onSubmit={(event) => {
        event.preventDefault();
        publish.mutate(body, {
          onSuccess: () => {
            setOpen(false);
            setName('');
            setTitle('');
            setPosition('');
            setBrief('');
            setNotes('');
            setNotebook(undefined);
            setTests(undefined);
            onPublished(body.name);
          },
        });
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-medium">Write a lab</h3>
        <Button
          size="sm"
          variant="ghost"
          type="button"
          className="ml-auto"
          onClick={() => setOpen(false)}
        >
          Cancel
        </Button>
      </div>

      <div className="grid gap-3 sm:grid-cols-3">
        <Field label="Name" hint="a-z, 0-9, dot, underscore, hyphen — it is the URL">
          <input
            className={INPUT}
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="lab-03"
            required
          />
        </Field>
        <Field label="Title">
          <input
            className={INPUT}
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="Answer the support questions"
            required
          />
        </Field>
        <Field label="Position" hint="1–999, or leave it unplaced">
          <input
            className={INPUT}
            value={position}
            inputMode="numeric"
            onChange={(event) => setPosition(event.target.value)}
            placeholder="3"
          />
        </Field>
      </div>

      <Field
        label="Notes"
        hint="What the participant reads. Kept verbatim — this panel renders no syntax."
      >
        <textarea
          className="id w-full resize-y rounded-md border border-border bg-transparent p-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
          rows={10}
          value={brief}
          onChange={(event) => setBrief(event.target.value)}
          required
        />
      </Field>
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex cursor-pointer items-center gap-1 text-xs text-primary hover:underline">
          <Upload className="h-3.5 w-3.5" /> Load the notes from a file
          <input
            type="file"
            accept=".md,.txt,.markdown,text/*"
            className="sr-only"
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = '';
              if (file) void readText(file).then(setBrief);
            }}
          />
        </label>
        <span className="text-xs text-muted-foreground">
          {new Blob([brief]).size.toLocaleString()} of 262,144 bytes
        </span>
      </div>

      <PinNotebook value={notebook} onChange={setNotebook} />
      <PinTests organization={organization} project={project} value={tests} onChange={setTests} />

      <Field label="Why this version exists" hint="optional — the commit message of a lab">
        <input
          className={INPUT}
          value={notes}
          onChange={(event) => setNotes(event.target.value)}
          placeholder="Reworded the second question."
        />
      </Field>

      <label className="flex items-center gap-2 text-xs">
        <input type="checkbox" checked={label} onChange={(e) => setLabel(e.target.checked)} />
        Make this the version participants read
        <span className="text-muted-foreground">
          — unchecked publishes a draft, which nobody is sent to
        </span>
      </label>

      {publish.error ? (
        <Refusal error={publish.error} fallback="this lab was not published" />
      ) : null}
      <Button type="submit" size="sm" className="self-start" disabled={!ready || publish.isPending}>
        {publish.isPending ? <Spinner /> : null} Publish
      </Button>
    </form>
  );
}

/**
 * The notebook the lab hands out, in whichever of its two shapes this lab is.
 *
 * **A file in a workshop** is the ordinary case: nine steps in a repository
 * with its own environment, opened by the workshop's own recipe. Nothing is
 * uploaded, because nothing here could run it — and no address is asked for,
 * because where marimo runs is the deployment's question or the participant's,
 * never the lab's.
 *
 * **A notebook this instance keeps** is the other: uploaded here, or one the
 * notebook runtime already holds. Both paths end in a name and the digest the
 * runtime answered, because that is the only pin this registry accepts for
 * bytes it keeps — a digest worked out in the browser would be a second content
 * address for a file the browser does not hold.
 */
function PinNotebook({
  value,
  onChange,
}: {
  value: LabNotebook | undefined;
  onChange: (notebook: LabNotebook | undefined) => void;
}) {
  const [shape, setShape] = React.useState<'none' | 'workshop' | 'kept'>('none');

  return (
    <div className="flex flex-col gap-2">
      <Field
        label="Notebook"
        hint="Optional. A lab may be a brief and its tests; this is for one that hands out something to work in."
      >
        <div className="flex flex-wrap gap-2">
          {(
            [
              ['none', 'None'],
              ['workshop', 'A file in a workshop'],
              ['kept', 'A notebook kept here'],
            ] as const
          ).map(([value_, label]) => (
            <Button
              key={value_}
              type="button"
              size="sm"
              variant={shape === value_ ? 'default' : 'outline'}
              onClick={() => {
                setShape(value_);
                onChange(undefined);
              }}
            >
              {label}
            </Button>
          ))}
        </div>
      </Field>
      {shape === 'workshop' ? <PinWorkshop value={value} onChange={onChange} /> : null}
      {shape === 'kept' ? <PinKept value={value} onChange={onChange} /> : null}
    </div>
  );
}

/**
 * A notebook in a repository the participants check out.
 *
 * Three authored facts and no address: which file, the command that opens it
 * where they have it, and the port that command lands on so the page can offer
 * an address without asking anybody to read one off their terminal. A URL here
 * would be authored data deciding where a browser goes.
 */
function PinWorkshop({
  value,
  onChange,
}: {
  value: LabNotebook | undefined;
  onChange: (notebook: LabNotebook) => void;
}) {
  const set = (next: Partial<LabNotebook>) =>
    onChange({
      path: next.path ?? value?.path ?? '',
      command: next.command ?? value?.command,
      port: next.port ?? value?.port,
    });

  return (
    <div className="grid gap-2 sm:grid-cols-3">
      <Field label="Path" hint="as the workshop names it">
        <input
          className={INPUT}
          value={value?.path ?? ''}
          onChange={(event) => set({ path: event.target.value.trim() })}
          placeholder="labs/00_foundations/00_llm_workflow_map/notebook.py"
        />
      </Field>
      <Field label="Command" hint="what a participant types">
        <input
          className={INPUT}
          value={value?.command ?? ''}
          onChange={(event) => set({ command: event.target.value })}
          placeholder="just notebook foundations 00_llm_workflow_map"
        />
      </Field>
      <Field label="Port" hint="where that command puts marimo">
        <input
          className={INPUT}
          value={value?.port ?? ''}
          inputMode="numeric"
          onChange={(event) => set({ port: Number(event.target.value) || undefined })}
          placeholder="2718"
        />
      </Field>
    </div>
  );
}

/**
 * A notebook this instance keeps: uploaded, or one the runtime already holds.
 *
 * Both paths end in the same place — a name and the digest the runtime
 * answered. Without a runtime there is no pin to be had, and saying so is
 * better than a field that takes a digest nobody can check.
 */
function PinKept({
  value,
  onChange,
}: {
  value: LabNotebook | undefined;
  onChange: (notebook: LabNotebook | undefined) => void;
}) {
  const notebooks = useQuery({
    queryKey: ['ml-pipeline', 'notebooks'],
    queryFn: getNotebooks,
    retry: false,
  });
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<unknown>(null);

  const upload = async (file: File | undefined, source?: string) => {
    setError(null);
    setBusy(true);
    try {
      const taken = new Set((notebooks.data ?? []).map((candidate) => candidate.name));
      const saved = await saveNotebook(
        freeName(file ? file.name : 'lab_notebook', taken),
        source ?? (await readText(file!)),
      );
      onChange({ path: saved.name, revision: saved.revision });
      await notebooks.refetch();
    } catch (failure) {
      setError(failure);
    } finally {
      setBusy(false);
    }
  };

  if (notebooks.error instanceof MlPipelineUnavailableError) {
    return (
      <p className="max-w-3xl text-xs text-muted-foreground">
        The notebook runtime is not running here, so there is nothing to keep one in. Start it with{' '}
        <span className="font-mono">just ml-pipeline-serve</span>, or hand out a file from a
        workshop instead.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      {value?.revision ? (
        <div className="flex flex-wrap items-center gap-2">
          <IdChip label="file" value={value.path} />
          <IdChip label="pinned" value={short(value.revision)} full={value.revision} />
          <Button size="sm" variant="ghost" type="button" onClick={() => onChange(undefined)}>
            Pick another
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex cursor-pointer items-center gap-1 text-xs text-primary hover:underline">
            <Upload className="h-3.5 w-3.5" /> Upload a marimo notebook
            <input
              type="file"
              accept=".py,text/x-python"
              className="sr-only"
              disabled={busy}
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = '';
                if (file) void upload(file);
              }}
            />
          </label>
          <Button
            size="sm"
            variant="outline"
            type="button"
            disabled={busy}
            onClick={() => void upload(undefined, EMPTY_NOTEBOOK_SOURCE)}
          >
            Start from the empty notebook
          </Button>
          <select
            className={INPUT}
            style={{ width: 'auto' }}
            value=""
            disabled={busy}
            onChange={(event) => {
              const chosen = notebooks.data?.find((n) => n.name === event.target.value);
              if (chosen) onChange({ path: chosen.name, revision: chosen.revision });
            }}
          >
            <option value="">…or one the runtime already holds</option>
            {notebooks.data?.map((candidate) => (
              <option key={candidate.name} value={candidate.name}>
                {candidate.name}
              </option>
            ))}
          </select>
          {busy ? <Spinner /> : null}
        </div>
      )}
      {error ? <Refusal error={error} fallback="the notebook was not saved" /> : null}
    </div>
  );
}

/**
 * A name the runtime does not already hold, derived from the uploaded file.
 *
 * Its rule, not ours: `^[a-z][a-z0-9_]{0,63}$`, so `Lab 03 — agents.py`
 * becomes `lab_03_agents`. A collision takes a suffix rather than the file,
 * because overwriting is how one workshop would break another's block.
 */
function freeName(fileName: string, taken: Set<string>): string {
  const stem = fileName
    .replace(/\.py$/i, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '');
  const base = (stem === '' ? 'lab_notebook' : /^[a-z]/.test(stem) ? stem : `lab_${stem}`).slice(
    0,
    64,
  );
  if (!taken.has(base)) return base;
  for (let attempt = 2; attempt < 100; attempt += 1) {
    const candidate = `${base.slice(0, 60)}_${attempt}`;
    if (!taken.has(candidate)) return candidate;
  }
  return `${base.slice(0, 55)}_${crypto.randomUUID().slice(0, 8).replace(/-/g, '')}`;
}

/**
 * What the work is measured by: a card at a version, and a cohort by its
 * digest.
 *
 * The card is picked at a **version** rather than at its head, because a
 * rewrite would otherwise change what an already-issued lab measures between
 * one participant's submission and the next. The cohort is typed rather than
 * chosen: cases are derived by `POST /evaluation-cohorts`, which answers the
 * digest, and there is no route that lists them — so a select here would be
 * a list this panel invented.
 */
function PinTests({
  organization,
  project,
  value,
  onChange,
}: {
  organization: string;
  project: string;
  value: LabTests | undefined;
  onChange: (tests: LabTests | undefined) => void;
}) {
  const cards = useQuery({
    queryKey: ['learning', 'scorecards', organization, project],
    retry: false,
    queryFn: async () =>
      answerOf(
        await projectListScorecards({ path: { organization, project } }),
        'this project did not answer its scorecards',
      ).scorecards,
  });
  const [card, setCard] = React.useState('');
  const versions = useQuery({
    queryKey: ['learning', 'scorecard-versions', organization, project, card],
    enabled: card !== '',
    retry: false,
    queryFn: async () =>
      answerOf(
        await projectListScorecardVersions({ path: { organization, project, name: card } }),
        'that scorecard did not answer its versions',
      ).versions,
  });

  const set = (next: Partial<LabTests>) =>
    onChange({
      scorecard: next.scorecard ?? value?.scorecard ?? { name: card, version: '' },
      cases: next.cases ?? value?.cases ?? '',
    });

  return (
    <div className="flex flex-col gap-2">
      <Field
        label="Tests"
        hint="Optional. A lab with none is one still being written — it says so rather than measuring nothing."
      >
        <div className="grid gap-2 sm:grid-cols-3">
          <select
            className={INPUT}
            value={card}
            onChange={(event) => {
              setCard(event.target.value);
              if (event.target.value === '') onChange(undefined);
            }}
          >
            <option value="">no scorecard</option>
            {cards.data?.map((head) => (
              <option key={head.name} value={head.name}>
                {head.name}
              </option>
            ))}
          </select>
          <select
            className={INPUT}
            disabled={card === ''}
            value={value?.scorecard.version ?? ''}
            onChange={(event) => set({ scorecard: { name: card, version: event.target.value } })}
          >
            <option value="">pick a version</option>
            {versions.data?.map((version) => (
              <option key={version.version} value={version.version}>
                {short(version.version)}
                {version.version === versions.data?.[0]?.version ? ' — newest' : ''}
              </option>
            ))}
          </select>
          <input
            className={INPUT}
            disabled={card === ''}
            value={value?.cases ?? ''}
            onChange={(event) => set({ cases: event.target.value.trim() })}
            placeholder="cohort digest"
          />
        </div>
      </Field>
      {cards.error ? (
        <Refusal error={cards.error} fallback="this project's scorecards could not be read" />
      ) : null}
      {value && value.cases.length > 0 && !/^[0-9a-f]{64}$/.test(value.cases) ? (
        <Badge tone="warning">
          a cohort is the 64-character digest{' '}
          <span className="font-mono">POST /evaluation-cohorts</span> answered
        </Badge>
      ) : null}
    </div>
  );
}

/**
 * A picked file, as text.
 *
 * `FileReader` rather than `Blob.text()`: both are the same thing in a
 * browser, and this one is also what the panel's test environment implements —
 * so the upload path that ships is the upload path the tests drive.
 */
function readText(file: File): Promise<string> {
  if (typeof file.text === 'function') return file.text();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('that file could not be read'));
    reader.onload = () => resolve(String(reader.result ?? ''));
    reader.readAsText(file);
  });
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
      <span className="text-xs font-medium">{label}</span>
      {children}
      {hint ? <span className="text-xs text-muted-foreground">{hint}</span> : null}
    </label>
  );
}
