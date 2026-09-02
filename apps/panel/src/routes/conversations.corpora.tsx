import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Copy, Square } from 'lucide-react';
import { z } from 'zod';

import {
  createConversationExport,
  cancelConversationExport,
  getConversationDatasetRows,
  listConversationDatasets,
  listConversationExports,
} from '@/api/generated';
import type { ExportFormat, ExportJobSummary, TrainingScope } from '@/api/generated/types.gen';
import { ArchiveDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Spinner,
  Stat,
} from '@/components/ui/primitives';

/**
 * What comes out of the archive: a job, and then an immutable corpus.
 *
 * The job is the point of this screen. An export of a real archive is minutes
 * of work, so it is queued rather than awaited, and what a reader needs while
 * it runs is not a spinner but the counts — how many turns it has considered,
 * how many it kept, and *why* it dropped the rest. An export that quietly
 * produced forty rows from four thousand turns looks exactly like one that
 * worked; the exclusion table is what turns that into "three thousand nine
 * hundred are still waiting for review".
 *
 * There is no progress bar for a queued job and there is one for a running one,
 * and the difference is honest: the conversation list is pinned when the job is
 * created, so the denominator is a fact rather than a guess. The training area
 * draws no bar for exactly the opposite reason — see `training.runs.tsx`.
 *
 * Polling stops when nothing is running, the same rule the training area keeps.
 */

const searchSchema = z.object({
  corpus: z.string().optional(),
  version: z.string().optional(),
});

export const Route = createFileRoute('/conversations/corpora')({
  validateSearch: searchSchema,
  component: CorporaPage,
});

const FORMAT_NOTES: Record<ExportFormat, string> = {
  chat: 'One row per conversation. The only lossless shape, and the one an unforeseen task can be rebuilt from.',
  prompt_response: 'One row per assistant turn, paired with the question. Loses tool use entirely.',
  sft: 'One row per assistant turn, with the preceding context kept.',
  dpo: 'One row per preference pair a reviewer explicitly labelled. Nothing is inferred.',
};

function CorporaPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = React.useState('training/agent-turns');
  const [format, setFormat] = React.useState<ExportFormat>('chat');
  const [scope, setScope] = React.useState<TrainingScope>('train');
  const [requireReview, setRequireReview] = React.useState(true);

  const jobs = useQuery({
    queryKey: ['conversation-exports'],
    retry: false,
    queryFn: async () => {
      const response = await listConversationExports({ throwOnError: true });
      return response.data.jobs;
    },
    // Only while something is moving. An archive with no running export is the
    // normal state, and polling it costs a request every few seconds forever.
    refetchInterval: (query) =>
      (query.state.data ?? []).some((job) => job.state === 'running' || job.state === 'queued')
        ? 3_000
        : false,
  });

  const corpora = useQuery({
    queryKey: ['conversation-datasets'],
    retry: false,
    queryFn: async () => {
      const response = await listConversationDatasets({ throwOnError: true });
      return response.data.exports;
    },
  });

  const queue = useMutation({
    mutationFn: async () => {
      const response = await createConversationExport({
        throwOnError: true,
        body: {
          name,
          format,
          required_scope: scope,
          require_human_review: requireReview,
        },
      });
      return response.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['conversation-exports'] });
    },
  });

  if (isRegistryDisabled(jobs.error) || isRegistryDisabled(corpora.error)) {
    return <ArchiveDisabled />;
  }

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Corpora</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          An export is a job: it pins what it will read, writes shards as it goes, and produces one
          immutable <code>name@sha256</code> a training run can name.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Queue an export</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-xs">
          <div className="flex flex-wrap items-end gap-3">
            <label className="flex flex-col gap-1">
              <span className="text-muted-foreground">Name</span>
              <input
                value={name}
                onChange={(event) => setName(event.target.value)}
                className="w-64 rounded border border-border bg-background px-2 py-1"
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-muted-foreground">Shape</span>
              <select
                value={format}
                onChange={(event) => setFormat(event.target.value as ExportFormat)}
                className="rounded border border-border bg-background px-2 py-1"
              >
                {(Object.keys(FORMAT_NOTES) as ExportFormat[]).map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-muted-foreground">Consent must permit</span>
              <select
                value={scope}
                onChange={(event) => setScope(event.target.value as TrainingScope)}
                className="rounded border border-border bg-background px-2 py-1"
              >
                {(['train', 'evaluate', 'share'] as TrainingScope[]).map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={requireReview}
                onChange={(event) => setRequireReview(event.target.checked)}
              />
              <span>Reviewed turns only</span>
            </label>
            <Button size="sm" disabled={queue.isPending} onClick={() => queue.mutate()}>
              Queue
            </Button>
          </div>
          <p className="text-muted-foreground">{FORMAT_NOTES[format]}</p>
          {!requireReview ? (
            <p className="text-warning">
              Unreviewed turns will be included. Nobody has read them for names, addresses or
              credentials.
            </p>
          ) : null}
          {queue.isError ? <p className="text-danger">{(queue.error as Error).message}</p> : null}
        </CardContent>
      </Card>

      <div className="flex flex-col gap-3">
        <h2 className="text-sm font-semibold">Jobs</h2>
        {jobs.isLoading ? <Spinner /> : null}
        {jobs.data?.length === 0 ? <EmptyState title="No export has been queued" /> : null}
        {jobs.data?.map((job) => (
          <JobCard key={job.job_id} job={job} />
        ))}
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="text-sm font-semibold">Immutable corpora</h2>
        {corpora.data?.length === 0 ? (
          <EmptyState
            title="Nothing has been frozen yet"
            hint="A job that completes writes a manifest; until then there is no version, which is what stops an interrupted export looking like a dataset."
          />
        ) : null}
        {corpora.data?.map((corpus) =>
          (corpus.versions ?? []).map((version) => (
            <Card key={`${corpus.name}@${version.version}`}>
              <CardHeader className="flex flex-row flex-wrap items-center gap-3">
                <CardTitle className="text-sm">{corpus.name}</CardTitle>
                <Badge tone="neutral">{version.format}</Badge>
                {version.withdrawn ? <Badge tone="warning">withdrawn</Badge> : null}
                <span className="text-xs text-muted-foreground">
                  {version.rows} {version.rows === 1 ? 'row' : 'rows'}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="ml-auto"
                  onClick={() => {
                    void navigator.clipboard.writeText(`${corpus.name}@${version.version}`);
                  }}
                >
                  <Copy className="mr-1 h-3 w-3" />
                  Copy reference
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    navigate({
                      search: { corpus: corpus.name, version: version.version },
                    })
                  }
                >
                  Rows
                </Button>
              </CardHeader>
              {search.corpus === corpus.name && search.version === version.version ? (
                <CardContent>
                  <RowPreview name={corpus.name} version={version.version} />
                </CardContent>
              ) : null}
            </Card>
          )),
        )}
      </div>
    </div>
  );
}

function JobCard({ job }: { job: ExportJobSummary }) {
  const queryClient = useQueryClient();
  const cancel = useMutation({
    mutationFn: async () => {
      const response = await cancelConversationExport({
        throwOnError: true,
        path: { job_id: job.job_id },
      });
      return response.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['conversation-exports'] });
    },
  });

  const running = job.state === 'running' || job.state === 'queued';
  const counts = job.counts ?? {
    conversations: 0,
    rows: 0,
    turns_considered: 0,
    turns_excluded: 0,
    turns_included: 0,
  };
  const fraction = job.conversations > 0 ? job.cursor / job.conversations : 0;

  return (
    <Card>
      <CardHeader className="flex flex-row flex-wrap items-center gap-3">
        <CardTitle className="text-sm">{job.name}</CardTitle>
        <Badge tone={stateTone(job.state)}>{job.state}</Badge>
        <Badge tone="neutral">{job.format}</Badge>
        {job.version ? (
          <code className="text-xs text-muted-foreground">@{job.version.slice(0, 12)}…</code>
        ) : null}
        {/* Which worker holds it. Answers "why has this been running for
            twenty minutes" without opening a log — and, when it changes, says
            that a pod was replaced rather than that the export stalled. */}
        {running && job.claimed_by ? (
          <span className="text-xs text-muted-foreground">on {job.claimed_by}</span>
        ) : null}
        {running ? (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            disabled={cancel.isPending}
            onClick={() => cancel.mutate()}
          >
            <Square className="mr-1 h-3 w-3" />
            Cancel
          </Button>
        ) : null}
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-xs">
        {running && job.conversations > 0 ? (
          <div className="h-1 w-full overflow-hidden rounded bg-muted">
            <div
              className="h-full bg-primary transition-[width]"
              style={{ width: `${Math.round(fraction * 100)}%` }}
            />
          </div>
        ) : null}
        <div className="flex flex-wrap gap-6">
          <Stat label="Conversations" value={`${job.cursor} / ${job.conversations}`} />
          <Stat label="Rows" value={counts.rows ?? 0} />
          <Stat label="Turns kept" value={counts.turns_included ?? 0} />
          <Stat label="Turns dropped" value={counts.turns_excluded ?? 0} />
        </div>
        {job.error ? <p className="text-danger">{job.error}</p> : null}
        <Exclusions jobId={job.job_id} />
      </CardContent>
    </Card>
  );
}

/**
 * Why rows did not make it, by reason.
 *
 * The counts come from the job summary's detail rather than the list, so this
 * fetches the job. Worth the request: the list is the thing people watch and
 * this is the thing they act on.
 */
function Exclusions({ jobId }: { jobId: string }) {
  const job = useQuery({
    queryKey: ['conversation-export', jobId],
    retry: false,
    queryFn: async () => {
      const { getConversationExport } = await import('@/api/generated');
      const response = await getConversationExport({
        throwOnError: true,
        path: { job_id: jobId },
      });
      return response.data;
    },
  });

  const exclusions = Object.entries(job.data?.exclusions ?? {});
  if (exclusions.length === 0) return null;

  return (
    <div className="flex flex-wrap items-center gap-2">
      <span className="text-muted-foreground">Left out:</span>
      {exclusions.map(([reason, count]) => (
        <Badge key={reason} tone={reason === 'not_reviewed' ? 'warning' : 'neutral'}>
          {count} {reason.replace(/_/g, ' ')}
        </Badge>
      ))}
    </div>
  );
}

function RowPreview({ name, version }: { name: string; version: string }) {
  const rows = useQuery({
    queryKey: ['conversation-dataset-rows', name, version],
    retry: false,
    queryFn: async () => {
      const response = await getConversationDatasetRows({
        throwOnError: true,
        query: { name, version, limit: 5 },
      });
      return response.data;
    },
  });

  if (rows.isLoading) return <Spinner />;
  if (rows.error) {
    const body = rows.error as { code?: string; message?: string };
    return (
      <p className="text-xs text-muted-foreground">
        {body.code === 'forbidden'
          ? 'Reading a corpus needs the admin role — its rows are conversation content.'
          : body.code === 'erased'
            ? 'An erasure withdrew this corpus. Its manifest, counts and exclusions remain; its rows do not.'
            : (body.message ?? 'Could not read this corpus.')}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-2 text-xs">
      <span className="text-muted-foreground">
        First {rows.data?.rows.length ?? 0} of {rows.data?.total ?? 0} rows
      </span>
      <pre className="max-h-96 overflow-auto rounded border border-border bg-muted/40 p-2">
        {(rows.data?.rows ?? []).map((row) => JSON.stringify(row, null, 2)).join('\n\n')}
      </pre>
    </div>
  );
}

function stateTone(state: string): 'success' | 'danger' | 'running' | 'warning' | 'neutral' {
  switch (state) {
    case 'completed':
      return 'success';
    case 'failed':
      return 'danger';
    case 'running':
      return 'running';
    case 'queued':
      return 'warning';
    default:
      return 'neutral';
  }
}
