import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { z } from 'zod';

import { getTrainingRun, listTrainingRuns } from '@/api/generated';
import type { TrainingRun, TrainingStatus } from '@/api/generated/types.gen';
import { LearningCurve, type CurveSeries } from '@/components/charts/learning-curve';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import {
  Badge,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
  Spinner,
  Stat,
} from '@/components/ui/primitives';
import { formatDuration } from '@/lib/utils';
import { cn } from '@/lib/utils';

/**
 * Training runs, and one run's curve.
 *
 * The list polls while anything is running and stops when nothing is — an
 * epoch is minutes, so a five-second poll is free and an SSE channel would be
 * machinery for a question that changes six times an hour.
 *
 * Two things on this page are deliberately not what a training dashboard
 * usually shows. There is no progress bar, because nothing here knows how many
 * epochs a run intends to do and a bar that guesses is a bar that lies. And a
 * run with no end is `running` with the time it was last heard from beside it,
 * never "stalled" — a trainer killed by an OOM and a trainer thinking for
 * twenty minutes are indistinguishable from here, and the panel draws the line
 * rather than the registry deciding.
 */

const STALLED_AFTER_MS = 15 * 60 * 1000;

const searchSchema = z.object({
  run: z.string().optional(),
  model: z.string().optional(),
  status: z.enum(['running', 'succeeded', 'failed', 'cancelled']).optional(),
  dataset: z.string().optional(),
  metrics: z.string().optional(),
  normalise: z.boolean().optional(),
});

export const Route = createFileRoute('/training/runs')({
  validateSearch: searchSchema,
  component: RunsPage,
});

const STATUS_TONES: Record<TrainingStatus, 'running' | 'success' | 'danger' | 'neutral'> = {
  running: 'running',
  succeeded: 'success',
  failed: 'danger',
  cancelled: 'neutral',
};

function RunsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  const runs = useQuery({
    queryKey: ['training-runs', search.model, search.status, search.dataset],
    queryFn: async () => {
      const response = await listTrainingRuns({
        throwOnError: true,
        query: {
          model: search.model,
          status: search.status,
          dataset: search.dataset,
          limit: 100,
        },
      });
      return response.data;
    },
    retry: false,
    // Only while something is actually moving. A page of finished runs that
    // polls every five seconds is a page that costs a request per reader per
    // five seconds for no new information.
    refetchInterval: (query) =>
      query.state.data?.runs.some((run) => run.status === 'running') ? 5_000 : false,
  });

  const runId = search.run ?? runs.data?.runs[0]?.run_id;

  const detail = useQuery({
    queryKey: ['training-run', runId],
    enabled: Boolean(runId),
    queryFn: async () => {
      const response = await getTrainingRun({
        throwOnError: true,
        path: { run_id: runId ?? '' },
      });
      return response.data;
    },
    refetchInterval: (query) => (query.state.data?.status === 'running' ? 5_000 : false),
  });

  if (runs.isError && isRegistryDisabled(runs.error)) {
    return <RegistryDisabled area="Training" />;
  }
  if (runs.isLoading) {
    return (
      <div className="flex justify-center p-10">
        <Spinner />
      </div>
    );
  }

  const update = (next: Partial<z.infer<typeof searchSchema>>) =>
    navigate({ search: (previous) => ({ ...previous, ...next }) });

  return (
    <div className="grid gap-3 lg:grid-cols-[20rem_1fr]">
      <Card className="flex max-h-[calc(100vh-11rem)] flex-col overflow-hidden">
        <div className="flex gap-1 border-b border-border p-2">
          <input
            defaultValue={search.model ?? ''}
            onChange={(event) => update({ model: event.target.value || undefined })}
            placeholder="model"
            className="min-w-0 flex-1 rounded-md border border-border bg-background px-2 py-1 text-xs"
          />
          <select
            value={search.status ?? ''}
            onChange={(event) =>
              update({ status: (event.target.value || undefined) as TrainingStatus | undefined })
            }
            className="rounded-md border border-border bg-background px-1.5 py-1 text-[11px]"
          >
            <option value="">any</option>
            {(['running', 'succeeded', 'failed', 'cancelled'] as const).map((status) => (
              <option key={status} value={status}>
                {status}
              </option>
            ))}
          </select>
        </div>
        <ul className="flex-1 overflow-y-auto p-1">
          {(runs.data?.runs ?? []).map((run) => (
            <li key={run.run_id}>
              <button
                type="button"
                onClick={() => update({ run: run.run_id })}
                className={cn(
                  'flex w-full flex-col gap-1 rounded-md px-2 py-1.5 text-left text-xs',
                  run.run_id === runId ? 'bg-accent' : 'hover:bg-accent/50',
                )}
              >
                <span className="flex items-center gap-2">
                  <Badge tone={STATUS_TONES[run.status]} className="px-1.5 py-0 text-[10px]">
                    {run.status}
                  </Badge>
                  <span className="truncate font-medium">{run.run_id}</span>
                </span>
                <span className="flex items-center gap-2 text-[10px] text-muted-foreground">
                  <span className="truncate">{run.model}</span>
                  <span>·</span>
                  <span>{run.epochs} epochs</span>
                  {run.best && (
                    <>
                      <span>·</span>
                      <span className="tabular-nums">
                        {run.best.metric} {run.best.value.toFixed(3)}
                      </span>
                    </>
                  )}
                </span>
                {!run.reproducible && (
                  <span className="text-[10px] text-warning">unversioned dataset</span>
                )}
              </button>
            </li>
          ))}
          {runs.data && runs.data.runs.length === 0 && (
            <EmptyState
              title="No training runs"
              hint="A trainer publishes one with aiwatcher_sdk.training.TrainingClient."
            />
          )}
        </ul>
      </Card>

      {detail.data ? (
        <RunDetail
          run={detail.data}
          selected={search.metrics?.split(',').filter(Boolean)}
          normalise={search.normalise ?? false}
          onSelect={(metrics) => update({ metrics: metrics.join(',') || undefined })}
          onNormalise={(normalise) => update({ normalise: normalise || undefined })}
        />
      ) : (
        <Card className="flex items-center justify-center p-10">
          {detail.isLoading ? <Spinner /> : <EmptyState title="Pick a run" />}
        </Card>
      )}
    </div>
  );
}

function RunDetail({
  run,
  selected,
  normalise,
  onSelect,
  onNormalise,
}: {
  run: TrainingRun;
  selected: string[] | undefined;
  normalise: boolean;
  onSelect: (metrics: string[]) => void;
  onNormalise: (normalise: boolean) => void;
}) {
  const names = React.useMemo(() => {
    const all = new Set<string>();
    for (const epoch of run.epochs ?? []) {
      for (const name of Object.keys(epoch.metrics)) all.add(name);
    }
    return [...all].sort();
  }, [run.epochs]);

  // Everything, until somebody narrows it. A first paint showing one metric
  // makes the reader hunt for the one they came for.
  const shown = selected?.length ? selected.filter((name) => names.includes(name)) : names;

  const series: CurveSeries[] = shown.map((name) => ({
    key: name,
    label: name,
    points: (run.epochs ?? [])
      .filter((epoch) => typeof epoch.metrics[name] === 'number')
      .map((epoch) => [epoch.epoch, epoch.metrics[name] as number] as [number, number]),
  }));

  const stalled =
    run.status === 'running' &&
    Date.now() - new Date(run.last_heard_from).getTime() > STALLED_AFTER_MS;

  return (
    <div className="flex flex-col gap-3">
      <Card>
        <CardHeader className="flex-row flex-wrap items-center gap-2">
          <CardTitle className="text-sm">{run.run_id}</CardTitle>
          <Badge tone={STATUS_TONES[run.status]}>{run.status}</Badge>
          {stalled && (
            <Badge tone="warning" title="Nothing has been written for fifteen minutes.">
              last heard from {formatDuration(Date.now() - new Date(run.last_heard_from).getTime())}{' '}
              ago
            </Badge>
          )}
          {!run.reproducible && (
            <Badge
              tone="warning"
              title="The dataset is a name rather than an immutable export reference, so nothing can reconstruct what this run learned from."
            >
              not reproducible
            </Badge>
          )}
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-xs">
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-muted-foreground sm:grid-cols-[auto_1fr_auto_1fr]">
            <dt>model</dt>
            <dd className="font-mono text-foreground">{run.model}</dd>
            <dt>dataset</dt>
            <dd className="truncate font-mono text-foreground" title={run.dataset}>
              {run.dataset}
            </dd>
            <dt>framework</dt>
            <dd className="text-foreground">{run.framework || '—'}</dd>
            <dt>device</dt>
            <dd className="text-foreground">{run.device || '—'}</dd>
            <dt>code</dt>
            <dd className="font-mono text-foreground">{run.code || '—'}</dd>
            <dt>started</dt>
            <dd className="text-foreground">{new Date(run.started_at).toLocaleString()}</dd>
          </dl>
          {run.error && <p className="text-danger">{run.error}</p>}
        </CardContent>
      </Card>

      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <Stat label="Epochs" value={String(run.epochs?.length ?? 0)} />
        <Stat
          label="Duration"
          value={
            run.ended_at
              ? formatDuration(
                  new Date(run.ended_at).getTime() - new Date(run.started_at).getTime(),
                )
              : '—'
          }
        />
        <Stat
          label={run.best?.metric ?? 'Best'}
          value={run.best ? run.best.value.toFixed(4) : '—'}
        />
        <Stat label="Checkpoints" value={String(run.checkpoints?.length ?? 0)} />
      </div>

      <Card>
        <CardHeader className="flex-row flex-wrap items-center justify-between gap-2">
          <CardTitle className="text-sm">Curve</CardTitle>
          <div className="flex flex-wrap items-center gap-1">
            {names.map((name) => (
              <button
                key={name}
                type="button"
                onClick={() =>
                  onSelect(
                    shown.includes(name) && shown.length > 1
                      ? shown.filter((entry) => entry !== name)
                      : [...new Set([...shown, name])],
                  )
                }
                className={cn(
                  'rounded-full border px-2 py-0.5 text-[11px] transition-colors',
                  shown.includes(name)
                    ? 'border-primary/40 bg-primary/10 text-foreground'
                    : 'border-border text-muted-foreground hover:text-foreground',
                )}
              >
                {name}
              </button>
            ))}
            <label className="ml-2 flex items-center gap-1 text-[11px] text-muted-foreground">
              <input
                type="checkbox"
                checked={normalise}
                onChange={(event) => onNormalise(event.target.checked)}
              />
              own scale
            </label>
          </div>
        </CardHeader>
        <CardContent>
          <LearningCurve series={series} normalise={normalise} />
          {!normalise && shown.length > 1 && (
            <p className="mt-1 text-[11px] text-muted-foreground">
              One scale. A loss at 1.6 and a score at 0.42 share an axis here — tick{' '}
              <em>own scale</em> to read their shapes instead of their magnitudes.
            </p>
          )}
        </CardContent>
      </Card>

      {(run.checkpoints?.length ?? 0) > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Checkpoints</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1 text-xs">
            {run.checkpoints?.map((checkpoint) => (
              <div
                key={checkpoint.uri}
                className="flex flex-wrap items-center gap-2 border-b border-border py-1 last:border-0"
              >
                {checkpoint.best && (
                  <Badge tone="success" className="px-1.5 py-0 text-[10px]">
                    best
                  </Badge>
                )}
                <IdChip value={checkpoint.uri} />
                {checkpoint.epoch !== undefined && checkpoint.epoch !== null && (
                  <span className="text-muted-foreground">epoch {checkpoint.epoch}</span>
                )}
                {checkpoint.metric && (
                  <span className="tabular-nums text-muted-foreground">
                    {checkpoint.metric} {checkpoint.value?.toFixed(4)}
                  </span>
                )}
              </div>
            ))}
          </CardContent>
        </Card>
      )}

      {(run.profiles?.length ?? 0) > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Profiler</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 text-xs">
            <p className="text-muted-foreground">
              The summary, not the trace. Sixty seconds of profiling emits more records than this
              projector holds for a week, and a profiler UI draws a flame graph better than a
              waterfall ever will.
            </p>
            {run.profiles?.map((profile, index) => (
              <ProfileSummary key={index} summary={profile.summary} uri={profile.uri} />
            ))}
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function ProfileSummary({ summary, uri }: { summary: unknown; uri?: string | null }) {
  const record = (summary ?? {}) as {
    top_share?: number;
    operators?: { name?: string; count?: number; self_cpu_us?: number }[];
  };
  return (
    <div className="flex flex-col gap-1 border-t border-border pt-2 first:border-0 first:pt-0">
      {typeof record.top_share === 'number' && (
        <span className="text-muted-foreground">
          the hottest operator is {(record.top_share * 100).toFixed(0)}% of self CPU time
        </span>
      )}
      {(record.operators ?? []).slice(0, 5).map((operator) => (
        <div key={operator.name} className="flex justify-between gap-4 font-mono text-[11px]">
          <span className="truncate">{operator.name}</span>
          <span className="tabular-nums text-muted-foreground">
            {operator.count} calls · {((operator.self_cpu_us ?? 0) / 1000).toFixed(1)} ms
          </span>
        </div>
      ))}
      {uri && (
        <a
          href={uri}
          className="text-primary hover:underline"
          target="_blank"
          rel="noreferrer noopener"
        >
          full trace
        </a>
      )}
    </div>
  );
}
