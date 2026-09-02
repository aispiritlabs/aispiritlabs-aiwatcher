import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Square } from 'lucide-react';
import { z } from 'zod';

import {
  cancelImportJob,
  listImportBatches,
  listImportJobs,
  listImportManifests,
  listImportRejects,
  queueImportJob,
} from '@/api/generated';
import type { ImportJob, RejectedRow, StagedBatch } from '@/api/generated/types.gen';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
  Spinner,
  Stat,
} from '@/components/ui/primitives';

/**
 * A corpus-sized import: the batch somebody staged, the job reading it, and
 * the rows it refused.
 *
 * The rejected rows are the reason this screen exists. An import of six
 * hundred thousand pictures that registered four hundred thousand of them
 * looks, from a success response, exactly like one that worked — and the two
 * hundred thousand it dropped are the whole story. So the counts come first,
 * grouped by reason, and the rows behind one reason are a click away.
 *
 * There *is* a progress bar here, unlike Training and like Conversations, and
 * for the same reason: the pages were counted when the batch was sealed, so
 * the denominator is a fact rather than a guess.
 *
 * Polling stops when nothing is moving, the same rule every other job view
 * keeps. See ADR_0022.
 */

const searchSchema = z.object({
  job: z.string().optional(),
});

export const Route = createFileRoute('/annotations/imports')({
  validateSearch: searchSchema,
  component: ImportsPage,
});

const STATE_TONE = {
  queued: 'neutral',
  running: 'running',
  completed: 'success',
  failed: 'danger',
  cancelled: 'warning',
} as const;

/** What a reason means, in the words of the thing that has to be fixed. */
const REASON_NOTES: Record<string, string> = {
  address_refused:
    'The address was refused before anything was downloaded: not https, not a host this instance may fetch from, or a name that resolves inside the network.',
  unreachable: 'The download failed, timed out, or answered something that was not a success.',
  not_an_image:
    'What came back was not a picture, was larger than the limits allow, or did not hash to what the row claimed.',
  invalid: 'The registry refused the row itself: no content address, a bad name, a zero dimension.',
  store_failed: 'The object store could not be written. This one is about here, not about the row.',
};

function ImportsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();

  const jobs = useQuery({
    queryKey: ['annotation-import-jobs'],
    retry: false,
    queryFn: async () => {
      const response = await listImportJobs({ throwOnError: true });
      return response.data.jobs;
    },
    refetchInterval: (query) =>
      (query.state.data ?? []).some((job) => job.state === 'running' || job.state === 'queued')
        ? 3_000
        : false,
  });

  const batches = useQuery({
    queryKey: ['annotation-import-batches'],
    retry: false,
    queryFn: async () => {
      const response = await listImportBatches({ throwOnError: true });
      return response.data.batches;
    },
  });

  const published = useQuery({
    queryKey: ['annotation-imports'],
    retry: false,
    queryFn: async () => {
      const response = await listImportManifests({ throwOnError: true });
      return response.data.imports;
    },
  });

  const queue = useMutation({
    mutationFn: async ({ batch, dryRun }: { batch: string; dryRun: boolean }) => {
      const response = await queueImportJob({
        throwOnError: true,
        body: { batch, dry_run: dryRun },
      });
      return response.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['annotation-import-jobs'] });
      void queryClient.invalidateQueries({ queryKey: ['annotation-import-batches'] });
    },
  });

  const stop = useMutation({
    mutationFn: async (jobId: string) => {
      await cancelImportJob({ throwOnError: true, query: { job_id: jobId } });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['annotation-import-jobs'] });
    },
  });

  if (isRegistryDisabled(jobs.error) || isRegistryDisabled(batches.error)) {
    return <RegistryDisabled area="Annotation imports" />;
  }

  const selected = jobs.data?.find((job) => job.job_id === search.job);
  const open = (job: ImportJob) =>
    void navigate({
      search: (previous) => ({
        ...previous,
        job: previous.job === job.job_id ? undefined : job.job_id,
      }),
    });

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Imports</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          A corpus is staged first — rows written to the store in pages — and then read by a job
          that survives the process that started it. Read the rejected rows before the count of
          accepted ones.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Staged batches</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {batches.isLoading ? <Spinner /> : null}
          {batches.data?.length === 0 ? (
            <EmptyState
              title="Nothing is staged"
              hint="Stage a batch with POST /api/v1/annotation-import-batches, append pages of rows, then queue it here. The synchronous route is still the right answer for a catalogue."
            />
          ) : null}
          {batches.data?.map((batch) => (
            <BatchRow
              key={batch.batch_id}
              batch={batch}
              onQueue={(dryRun) => queue.mutate({ batch: batch.batch_id, dryRun })}
              queuing={queue.isPending}
            />
          ))}
          {queue.error ? (
            <p className="text-sm text-danger">{String((queue.error as Error).message)}</p>
          ) : null}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Jobs</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {jobs.isLoading ? <Spinner /> : null}
          {jobs.data?.length === 0 ? <EmptyState title="No import has been queued" /> : null}
          {jobs.data?.map((job) => (
            <JobRow
              key={job.job_id}
              job={job}
              expanded={job.job_id === search.job}
              onOpen={() => open(job)}
              onCancel={() => stop.mutate(job.job_id)}
            />
          ))}
        </CardContent>
      </Card>

      {selected ? <Rejects job={selected} /> : null}

      <Card>
        <CardHeader>
          <CardTitle>Published imports</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {published.data?.length === 0 ? (
            <EmptyState
              title="Nothing published yet"
              hint="A cancelled or failed job writes no manifest, which is what stops an interrupted import appearing as a finished one."
            />
          ) : null}
          {published.data?.map((entry) => (
            <div
              key={entry.version}
              className="flex items-center justify-between gap-3 rounded-md border border-border px-3 py-2 text-sm"
            >
              <span className="font-medium">{entry.project}</span>
              <IdChip value={entry.version} />
              <span className="text-muted-foreground">
                {entry.accepted} registered · {entry.rejected} refused
              </span>
            </div>
          ))}
        </CardContent>
      </Card>
    </div>
  );
}

function BatchRow({
  batch,
  onQueue,
  queuing,
}: {
  batch: StagedBatch;
  onQueue: (dryRun: boolean) => void;
  queuing: boolean;
}) {
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-md border border-border px-3 py-2 text-sm">
      <span className="font-medium">{batch.project}</span>
      <IdChip value={batch.batch_id} />
      <span className="text-muted-foreground">
        {batch.rows} rows · {batch.pages.length} pages
      </span>
      <Badge tone={batch.sealed ? 'neutral' : 'running'}>{batch.sealed ? 'sealed' : 'open'}</Badge>
      <Badge tone={batch.rights.kind === 'unknown' ? 'warning' : 'neutral'}>
        {batch.rights.kind}
      </Badge>
      {batch.source.revision ? (
        <span className="text-xs text-muted-foreground">
          at {batch.source.revision.slice(0, 12)}
        </span>
      ) : (
        <span className="text-xs text-warning">no revision pinned</span>
      )}
      <div className="ml-auto flex gap-2">
        {/* Always offer the dry run first. Six hundred thousand images with the
            split key mapped from a filename is not something to discover after
            the fact, and a dry run costs the downloads and nothing else. */}
        <Button size="sm" variant="outline" disabled={queuing} onClick={() => onQueue(true)}>
          Dry run
        </Button>
        <Button size="sm" disabled={queuing} onClick={() => onQueue(false)}>
          {batch.sealed ? 'Import again' : 'Import'}
        </Button>
      </div>
    </div>
  );
}

function JobRow({
  job,
  expanded,
  onOpen,
  onCancel,
}: {
  job: ImportJob;
  expanded: boolean;
  onOpen: () => void;
  onCancel: () => void;
}) {
  const done = job.pages === 0 ? 0 : Math.round((job.cursor / job.pages) * 100);
  const running = job.state === 'running' || job.state === 'queued';
  return (
    <div className="rounded-md border border-border">
      {/* A row rather than one big button: `IdChip` is itself a button (it
          copies), and a button inside a button is invalid HTML that React
          reports as a hydration error. The expander is its own control. */}
      <div className="flex flex-wrap items-center gap-3 px-3 py-2 text-sm">
        <button
          type="button"
          onClick={onOpen}
          aria-expanded={expanded}
          className="flex items-center gap-3 text-left"
        >
          <Badge tone={STATE_TONE[job.state]}>{job.state}</Badge>
          <span className="font-medium">{job.project}</span>
        </button>
        {job.request.dry_run ? <Badge tone="neutral">dry run</Badge> : null}
        <IdChip value={job.job_id} />
        <span className="ml-auto text-muted-foreground">
          {job.counts.accepted} registered · {job.counts.rejected} refused
        </span>
        {running ? (
          <button
            type="button"
            title="Stop this import"
            onClick={onCancel}
            className="flex h-8 w-8 items-center justify-center text-muted-foreground hover:text-danger"
          >
            <Square className="h-3.5 w-3.5" />
          </button>
        ) : null}
      </div>
      {running ? (
        <div className="px-3 pb-2">
          <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
            <div className="h-full bg-running transition-all" style={{ width: `${done}%` }} />
          </div>
          <p className="pt-1 text-xs text-muted-foreground">
            page {job.cursor} of {job.pages}
            {job.claimed_by ? ` · held by ${job.claimed_by}` : null}
          </p>
        </div>
      ) : null}
      {expanded ? (
        <div className="flex flex-col gap-3 border-t border-border px-3 py-3">
          <div className="flex flex-wrap gap-6">
            <Stat label="Rows considered" value={job.counts.rows_considered} />
            <Stat label="Registered" value={job.counts.accepted} />
            <Stat label="Refused" value={job.counts.rejected} />
            <Stat label="Downloaded" value={job.counts.fetched} />
            <Stat label="Version" value={job.version ? job.version.slice(0, 12) : '—'} />
          </div>
          {job.warnings.map((warning) => (
            <p key={warning} className="text-sm text-warning">
              {warning}
            </p>
          ))}
          {job.error ? <p className="text-sm text-danger">{job.error}</p> : null}
        </div>
      ) : null}
    </div>
  );
}

function Rejects({ job }: { job: ImportJob }) {
  const rows = useQuery({
    queryKey: ['annotation-import-rejects', job.job_id],
    retry: false,
    queryFn: async () => {
      const response = await listImportRejects({
        throwOnError: true,
        query: { job_id: job.job_id, limit: 100 },
      });
      return response.data;
    },
  });

  const reasons = Object.entries(job.rejects);
  if (reasons.length === 0) {
    return null;
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>What it refused</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="flex flex-wrap gap-2">
          {reasons.map(([reason, count]) => (
            <div key={reason} className="rounded-md border border-border px-3 py-2 text-sm">
              <div className="flex items-center gap-2">
                <Badge tone="danger">{count}</Badge>
                <span className="font-medium">{reason.replaceAll('_', ' ')}</span>
              </div>
              <p className="max-w-md pt-1 text-xs text-muted-foreground">
                {REASON_NOTES[reason] ?? ''}
              </p>
            </div>
          ))}
        </div>
        {rows.isLoading ? <Spinner /> : null}
        <div className="flex flex-col gap-1">
          {rows.data?.rows.map((row: RejectedRow, index: number) => (
            <div
              key={`${row.page}-${row.uri}-${index}`}
              className="rounded-md border border-border px-3 py-2 text-xs"
            >
              <div className="flex flex-wrap items-center gap-2">
                <Badge tone="danger">{row.reason}</Badge>
                <span className="font-mono">{row.uri}</span>
                <span className="text-muted-foreground">group {row.group_id}</span>
              </div>
              <p className="pt-1 text-muted-foreground">{row.detail}</p>
            </div>
          ))}
        </div>
        {rows.data && rows.data.total > rows.data.rows.length ? (
          <p className="text-xs text-muted-foreground">
            Showing {rows.data.rows.length} of {rows.data.total}. The counts above are complete.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
