import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Search } from 'lucide-react';
import { z } from 'zod';

import { getEvaluation, listEvaluationSuites, listEvaluations } from '@/api/generated/sdk.gen';
import type {
  EvaluationCase,
  EvaluationDetail,
  EvaluationSummary,
  MetricDelta,
  SuiteSummary,
} from '@/api/generated/types.gen';
import { Badge, Button, Card, EmptyState, IdChip, Spinner, Stat } from '@/components/ui/primitives';
import { StatusBadge } from '@/components/status-badge';
import {
  DEFAULT_WINDOW_SECONDS,
  TimeRange,
  windowParam,
  windowSearchSchema,
} from '@/components/time-range';
import { VirtualList } from '@/components/virtual-list';
import { cn, formatDuration, formatTime, pinchId } from '@/lib/utils';

/**
 * Scoring the thing the traces come from.
 *
 * ```text
 * suite (on a dataset)  ── the level MLflow calls an experiment
 * └── report                = one execution: params in, metrics and a document out
 *     ├── cases             = one score each, with the rationale that produced it
 *     └── comparison        = against the previous report on the same dataset
 * ```
 *
 * Everything here is folded from the same event log the traces come from —
 * `eval.*` events produce no span and no row in the runs list. See
 * `crates/aiwatcher-projector/src/evaluations.rs`.
 *
 * ## Why the metric deltas are not coloured
 *
 * Higher is better for a pass rate and worse for a cost, and this page has no
 * way to know which a producer's metric is. Colouring them would mean guessing,
 * and a green number that means "we got more expensive" is worse than a plain
 * one. What *is* unambiguous is a case that passed on the baseline and fails
 * now, so that is the thing that gets a colour.
 */

const searchSchema = z.object({
  ...windowSearchSchema,
  suite: z.string().optional(),
  dataset: z.string().optional(),
  status: z.enum(['running', 'succeeded', 'failed']).optional(),
  q: z.string().optional(),
  /** The report open in the pane on the right. */
  report: z.string().optional(),
});

export const Route = createFileRoute('/evaluation')({
  validateSearch: searchSchema,
  component: EvaluationPage,
});

const REPORT_PAGE = 50;

function EvaluationPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const select = React.useCallback(
    (next: Partial<z.infer<typeof searchSchema>>) => {
      void navigate({ search: (previous) => ({ ...previous, ...next }) });
    },
    [navigate],
  );

  const suites = useQuery({
    queryKey: ['evaluation-suites'],
    queryFn: async () => {
      const response = await listEvaluationSuites();
      if (!response.data) throw new Error('failed to load suites');
      return response.data;
    },
    refetchInterval: 15_000,
  });

  const reports = useInfiniteQuery({
    queryKey: ['evaluations', search.suite, search.dataset, search.status, search.q, windowSeconds],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listEvaluations({
        query: {
          suite: search.suite,
          dataset: search.dataset,
          status: search.status,
          search: search.q || undefined,
          // A report is dated by when it finished, not when it started: a
          // twenty-minute batch is normal here. See the projector's `window`.
          window_seconds: windowParam(windowSeconds),
          after: pageParam,
          limit: REPORT_PAGE,
        },
      });
      if (!response.data) throw new Error('failed to list evaluations');
      return response.data;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    // A suite that runs on a schedule finishes while nobody is looking. Cheap
    // to poll, and the alternative is a stale page that looks like a quiet one.
    refetchInterval: 10_000,
  });

  const rows = React.useMemo(
    () => reports.data?.pages.flatMap((page) => page.evaluations) ?? [],
    [reports.data],
  );
  const total = reports.data?.pages[0]?.total_known ?? 0;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Evaluation</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Scoring runs against a dataset: which prompt, model or agent version answers better, and
            by how much. Reports arrive on the same log as the traces and are folded apart from
            them.
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(seconds) =>
            void navigate({ search: (previous) => ({ ...previous, window: seconds }) })
          }
        />
      </div>

      <Suites
        suites={suites.data?.suites ?? []}
        loading={suites.isLoading}
        selected={{ suite: search.suite, dataset: search.dataset }}
        onSelect={(suite, dataset) =>
          select(
            search.suite === suite && search.dataset === (dataset ?? undefined)
              ? { suite: undefined, dataset: undefined, report: undefined }
              : { suite, dataset: dataset ?? undefined, report: undefined },
          )
        }
      />

      <Filters
        search={search}
        onSelect={select}
        summary={reports.isLoading ? 'loading…' : `${total} report${total === 1 ? '' : 's'}`}
      />

      <div className="grid gap-4 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
        <Card className="overflow-hidden">
          {reports.isError ? (
            <EmptyState
              title="Could not reach the API"
              hint="Is the aiwatcher server running? The panel proxies /api to it in development."
            />
          ) : rows.length === 0 && !reports.isLoading ? (
            <EmptyState
              title="No evaluation reports in this window"
              hint={
                windowSeconds
                  ? 'Nothing finished in the selected period. Widen it, or pick “all”.'
                  : "Publish an eval.completed event — the Python SDK's record_evaluation is one call — and it will appear here."
              }
            />
          ) : (
            <VirtualList
              items={rows}
              className="max-h-[34rem]"
              estimateSize={62}
              keyOf={(row) => row.evaluation_id}
              onReachEnd={() => {
                if (reports.hasNextPage && !reports.isFetchingNextPage) {
                  void reports.fetchNextPage();
                }
              }}
              isFetchingMore={reports.isFetchingNextPage}
              renderRow={(row) => (
                <ReportRow
                  report={row}
                  selected={row.evaluation_id === search.report}
                  onSelect={() => select({ report: row.evaluation_id })}
                />
              )}
            />
          )}
        </Card>

        <ReportPane evaluationId={search.report} />
      </div>
    </div>
  );
}

// ── Suites ───────────────────────────────────────────────────────────────────

function Suites({
  suites,
  loading,
  selected,
  onSelect,
}: {
  suites: SuiteSummary[];
  loading: boolean;
  selected: { suite?: string; dataset?: string };
  onSelect: (suite: string, dataset: string | null | undefined) => void;
}) {
  if (loading) {
    return (
      <p className="flex items-center gap-2 text-xs text-muted-foreground">
        <Spinner />
        loading suites
      </p>
    );
  }
  if (suites.length === 0) return null;

  return (
    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      {suites.map((suite) => {
        const active =
          selected.suite === suite.suite && (selected.dataset ?? null) === (suite.dataset ?? null);
        return (
          <button
            key={`${suite.suite}::${suite.dataset ?? ''}`}
            type="button"
            onClick={() => onSelect(suite.suite, suite.dataset)}
            className={cn(
              'rounded-lg border border-border bg-card p-4 text-left transition-colors hover:bg-accent/40',
              active && 'border-primary',
            )}
          >
            <div className="flex items-start justify-between gap-2">
              <div className="min-w-0">
                <p className="truncate text-sm font-medium">{suite.suite}</p>
                <p className="truncate text-xs text-muted-foreground">
                  {suite.dataset ?? 'no dataset named'}
                </p>
              </div>
              <StatusBadge status={suite.last_status} />
            </div>
            <div className="mt-3 flex items-end justify-between gap-3">
              <Stat
                label="Pass rate"
                value={formatRate(suite.pass_rate)}
                hint={`${suite.evaluations} report${suite.evaluations === 1 ? '' : 's'}`}
              />
              <div className="flex flex-col items-end gap-0.5 text-xs">
                {Object.entries(suite.latest_metrics)
                  .slice(0, 3)
                  .map(([name, value]) => (
                    <span key={name} className="tabular-nums text-muted-foreground">
                      {name} <span className="text-foreground">{formatMetric(value)}</span>
                      <Delta value={suite.metric_deltas[name]} />
                    </span>
                  ))}
              </div>
            </div>
          </button>
        );
      })}
    </div>
  );
}

// ── Filters ──────────────────────────────────────────────────────────────────

function Filters({
  search,
  onSelect,
  summary,
}: {
  search: z.infer<typeof searchSchema>;
  onSelect: (next: Partial<z.infer<typeof searchSchema>>) => void;
  summary: string;
}) {
  const [draft, setDraft] = React.useState(search.q ?? '');
  const q = search.q ?? '';

  // The URL is the state; the input is a draft of it. Same rule as the
  // explorer's search boxes: commit on a debounce, so the history does not
  // fill with half-typed words.
  React.useEffect(() => setDraft(q), [q]);
  React.useEffect(() => {
    if (draft === q) return;
    const timer = setTimeout(() => onSelect({ q: draft || undefined }), 250);
    return () => clearTimeout(timer);
  }, [draft, q, onSelect]);

  return (
    <div className="flex flex-wrap items-center gap-3">
      <label className="relative flex-1 md:max-w-sm">
        <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Search suite, dataset, variant or a parameter"
          className="h-8 w-full rounded-md border border-border bg-background pl-7 pr-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
        />
      </label>
      <div className="flex items-center gap-2">
        {(['running', 'succeeded', 'failed'] as const).map((status) => (
          <Button
            key={status}
            size="sm"
            variant={search.status === status ? 'default' : 'outline'}
            onClick={() => onSelect({ status: search.status === status ? undefined : status })}
          >
            {status}
          </Button>
        ))}
      </div>
      {search.suite ? (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onSelect({ suite: undefined, dataset: undefined })}
        >
          clear suite
        </Button>
      ) : null}
      <span className="text-xs text-muted-foreground">{summary}</span>
    </div>
  );
}

// ── The list ─────────────────────────────────────────────────────────────────

function ReportRow({
  report,
  selected,
  onSelect,
}: {
  report: EvaluationSummary;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'w-full border-b border-border/40 px-3 py-2 text-left transition-colors hover:bg-accent/40',
        selected && 'bg-accent/60',
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-sm font-medium">{report.suite}</span>
        <StatusBadge status={report.status} />
      </div>
      <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
        <span className="tabular-nums">{formatTime(report.started_at)}</span>
        <span>·</span>
        <span className="tabular-nums">{formatDuration(report.duration_ms)}</span>
        <span>·</span>
        <span className="tabular-nums">{formatRate(report.pass_rate)}</span>
        {report.dataset ? (
          <>
            <span>·</span>
            <span className="truncate">{report.dataset}</span>
          </>
        ) : null}
      </div>
    </button>
  );
}

// ── The detail pane ──────────────────────────────────────────────────────────

function ReportPane({ evaluationId }: { evaluationId?: string }) {
  const detail = useQuery({
    queryKey: ['evaluation', evaluationId],
    enabled: Boolean(evaluationId),
    queryFn: async () => {
      const response = await getEvaluation({ path: { evaluation_id: evaluationId! } });
      if (!response.data) throw new Error('failed to load the evaluation');
      return response.data;
    },
    refetchInterval: 10_000,
  });

  if (!evaluationId) {
    return (
      <Card>
        <EmptyState
          title="Select a report"
          hint="Its parameters, metrics, per-case scores and the document the producer attached."
        />
      </Card>
    );
  }
  if (detail.isLoading) {
    return (
      <Card>
        <EmptyState title="Loading…" />
      </Card>
    );
  }
  if (!detail.data) {
    return (
      <Card>
        <EmptyState
          title="That report is no longer retained"
          hint="Evaluations are held in a bounded projection; the events themselves are still in the log."
        />
      </Card>
    );
  }

  return <ReportDetail detail={detail.data} />;
}

function ReportDetail({ detail }: { detail: EvaluationDetail }) {
  const { summary, comparison } = detail;

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <Card className="p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h2 className="truncate text-base font-semibold">{summary.suite}</h2>
              <StatusBadge status={summary.status} />
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <IdChip
                label="evaluation"
                value={pinchId(summary.evaluation_id, 10, 8)}
                full={summary.evaluation_id}
              />
              {summary.dataset ? <Badge>{summary.dataset}</Badge> : null}
              {summary.variant ? <Badge tone="warning">{summary.variant}</Badge> : null}
              <span>{summary.runtime}</span>
            </div>
          </div>
        </div>

        <div className="mt-4 grid grid-cols-2 gap-4 md:grid-cols-4">
          <Stat label="Pass rate" value={formatRate(summary.pass_rate)} />
          <Stat
            label="Cases"
            value={summary.cases_total}
            hint={`${summary.cases_passed} passed · ${summary.cases_failed} failed`}
          />
          <Stat label="Duration" value={formatDuration(summary.duration_ms)} />
          <Stat label="Started" value={formatTime(summary.started_at)} />
        </div>

        {summary.error ? (
          <p className="mt-3 rounded-md bg-danger/10 px-3 py-2 text-xs text-danger">
            {summary.error}
          </p>
        ) : null}
      </Card>

      <div className="grid gap-4 xl:grid-cols-2">
        <Metrics
          metrics={summary.metrics}
          comparison={comparison?.metrics}
          baselineId={comparison?.baseline_id}
        />
        <Params params={summary.params} />
      </div>

      {comparison ? <Regressions detail={detail} /> : null}

      <Cases detail={detail} />

      <ReportDocument detail={detail} />
    </div>
  );
}

function Metrics({
  metrics,
  comparison,
  baselineId,
}: {
  metrics: Record<string, number>;
  comparison?: MetricDelta[];
  baselineId?: string;
}) {
  // With a baseline, the comparison already lists every metric either side
  // reported — including one that appeared or disappeared, which is exactly
  // what a plain map of this report's metrics would hide.
  const rows: MetricDelta[] =
    comparison ?? Object.entries(metrics).map(([name, value]) => ({ name, current: value }));

  return (
    <Card>
      <div className="flex items-center justify-between p-4 pb-2">
        <h3 className="text-sm font-semibold">Metrics</h3>
        {baselineId ? (
          <span className="text-xs text-muted-foreground">
            against <IdChip value={pinchId(baselineId, 8, 6)} full={baselineId} />
          </span>
        ) : null}
      </div>
      {rows.length === 0 ? (
        <p className="px-4 pb-4 text-xs text-muted-foreground">Nothing was measured.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <tbody>
            {rows.map((row) => (
              <tr key={row.name} className="border-t border-border/40">
                <td className="px-4 py-1.5 text-muted-foreground">{row.name}</td>
                <td className="px-4 py-1.5 text-right tabular-nums">
                  {row.current === undefined || row.current === null
                    ? '—'
                    : formatMetric(row.current)}
                </td>
                <td className="w-24 px-4 py-1.5 text-right text-xs tabular-nums">
                  <Delta value={row.delta} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

function Params({ params }: { params: Record<string, string> }) {
  const entries = Object.entries(params);
  return (
    <Card>
      <h3 className="p-4 pb-2 text-sm font-semibold">Parameters</h3>
      {entries.length === 0 ? (
        <p className="px-4 pb-4 text-xs text-muted-foreground">Nothing was pinned.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <tbody>
            {entries.map(([name, value]) => (
              <tr key={name} className="border-t border-border/40">
                <td className="px-4 py-1.5 text-muted-foreground">{name}</td>
                <td className="px-4 py-1.5 text-right font-medium">{value}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

function Regressions({ detail }: { detail: EvaluationDetail }) {
  const comparison = detail.comparison;
  if (!comparison) return null;
  if (comparison.regressed.length === 0 && comparison.fixed.length === 0) return null;

  return (
    <Card>
      <div className="p-4 pb-2">
        <h3 className="text-sm font-semibold">Changed cases</h3>
        <p className="text-xs text-muted-foreground">
          Against {comparison.baseline_id}, run {formatTime(comparison.baseline_started_at)}. A case
          that passed then and fails now is the one thing on this page whose direction is not a
          matter of interpretation.
        </p>
      </div>
      <div className="grid gap-4 p-4 pt-2 md:grid-cols-2">
        <CaseDeltas title="Regressed" tone="danger" cases={comparison.regressed} />
        <CaseDeltas title="Fixed" tone="success" cases={comparison.fixed} />
      </div>
    </Card>
  );
}

function CaseDeltas({
  title,
  tone,
  cases,
}: {
  title: string;
  tone: 'danger' | 'success';
  cases: { case_id: string; current_score?: number | null; baseline_score?: number | null }[];
}) {
  return (
    <div>
      <div className="flex items-center gap-2">
        <Badge tone={tone}>{cases.length}</Badge>
        <span className="text-xs font-medium">{title}</span>
      </div>
      <ul className="mt-2 flex flex-col gap-1">
        {cases.slice(0, 20).map((item) => (
          <li key={item.case_id} className="flex items-center justify-between gap-2 text-xs">
            <span className="truncate">{item.case_id}</span>
            <span className="tabular-nums text-muted-foreground">
              {item.baseline_score === undefined || item.baseline_score === null
                ? '—'
                : formatMetric(item.baseline_score)}{' '}
              →{' '}
              {item.current_score === undefined || item.current_score === null
                ? '—'
                : formatMetric(item.current_score)}
            </span>
          </li>
        ))}
        {cases.length > 20 ? (
          <li className="text-xs text-muted-foreground">and {cases.length - 20} more</li>
        ) : null}
      </ul>
    </div>
  );
}

function Cases({ detail }: { detail: EvaluationDetail }) {
  if (detail.cases.length === 0) {
    return (
      <Card>
        <h3 className="p-4 pb-2 text-sm font-semibold">Cases</h3>
        <p className="px-4 pb-4 text-xs text-muted-foreground">
          {detail.summary.cases_total > 0
            ? 'This evaluation reported totals rather than a case each, or its cases were shed to keep the projection inside its budget. The counts above are still the ones it reported.'
            : 'No per-case results were sent.'}
        </p>
      </Card>
    );
  }

  return (
    <Card className="overflow-hidden">
      <div className="flex items-center justify-between p-4 pb-2">
        <h3 className="text-sm font-semibold">Cases</h3>
        {detail.cases_truncated ? (
          <span className="text-xs text-muted-foreground">
            showing {detail.cases.length} of {detail.summary.cases_total}
          </span>
        ) : null}
      </div>
      <VirtualList
        items={detail.cases}
        className="max-h-[24rem]"
        estimateSize={40}
        keyOf={(item, index) => `${item.case_id}-${index}`}
        renderRow={(item) => <CaseRow item={item} />}
      />
    </Card>
  );
}

function CaseRow({ item }: { item: EvaluationCase }) {
  return (
    <div className="flex items-start justify-between gap-3 border-t border-border/40 px-4 py-2">
      <div className="min-w-0">
        <p className="truncate text-sm">{item.case_id}</p>
        {item.reason || item.error ? (
          <p className="truncate text-xs text-muted-foreground">{item.error ?? item.reason}</p>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <span className="tabular-nums text-sm">
          {item.score === undefined || item.score === null ? '—' : formatMetric(item.score)}
        </span>
        {item.passed === undefined || item.passed === null ? null : (
          <Badge tone={item.passed ? 'success' : 'danger'}>{item.passed ? 'pass' : 'fail'}</Badge>
        )}
      </div>
    </div>
  );
}

function ReportDocument({ detail }: { detail: EvaluationDetail }) {
  const { report, summary } = detail;
  if (!report) {
    if (summary.report_bytes === 0) return null;
    return (
      <Card>
        <h3 className="p-4 pb-2 text-sm font-semibold">Report</h3>
        <p className="px-4 pb-4 text-xs text-muted-foreground">
          The document was {summary.report_bytes.toLocaleString()} bytes and is not held — either
          over the per-report cap, or shed to keep the projection inside its budget. It is still in
          the event log.
        </p>
      </Card>
    );
  }

  return (
    <Card>
      <details>
        <summary className="cursor-pointer p-4 text-sm font-semibold">
          Report{' '}
          <span className="font-normal text-muted-foreground">
            ({summary.report_bytes.toLocaleString()} bytes)
          </span>
        </summary>
        <pre className="max-h-[24rem] overflow-auto border-t border-border/40 p-4 text-xs">
          {JSON.stringify(report, null, 2)}
        </pre>
      </details>
    </Card>
  );
}

// ── Formatting ───────────────────────────────────────────────────────────────

function formatRate(rate: number | null | undefined): string {
  if (rate === null || rate === undefined) return '—';
  return `${(rate * 100).toFixed(1)}%`;
}

/** Enough digits to be useful, few enough to line up in a column. */
function formatMetric(value: number): string {
  if (!Number.isFinite(value)) return '—';
  if (Number.isInteger(value)) return String(value);
  const magnitude = Math.abs(value);
  if (magnitude < 0.001) return value.toExponential(2);
  return value
    .toFixed(magnitude < 1 ? 4 : 2)
    .replace(/0+$/, '')
    .replace(/\.$/, '');
}

function Delta({ value }: { value: number | null | undefined }) {
  if (value === null || value === undefined || value === 0) return null;
  return (
    <span className="ml-1 text-muted-foreground">
      ({value > 0 ? '+' : ''}
      {formatMetric(value)})
    </span>
  );
}
