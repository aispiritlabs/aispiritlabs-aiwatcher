import { LocalViews } from '@/shared/components/local-views';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { getRouteApi } from '@tanstack/react-router';
import { Search } from 'lucide-react';
import * as React from 'react';
import { z } from 'zod';
import { searchSchema } from './search';
import { ParameterComparison } from '@/shared/components/parameter-comparison';
import { DatasetReference, ExecutionReference } from '@/shared/components/lineage-reference';

import { getEvaluation, listEvaluations, listEvaluationSuites } from '@/api/generated/sdk.gen';
import type {
  Comparability,
  EvaluationCase,
  EvaluationDetail,
  EvaluationSummary,
  MetricDelta,
  SuiteSummary,
} from '@/api/generated/types.gen';
import { StatusBadge } from '@/shared/components/status-badge';
import { Approvals } from './approvals';
import { EvidencePane, EvidenceRow, EvidenceUnavailable, Retention, useEvidence } from './evidence';
import type { DurableEvaluation } from '@/api/generated/types.gen';
import { ApiFailure } from '@/shared/lib/result';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  IdChip,
  Spinner,
  Stat,
} from '@/shared/components/ui/primitives';
import { TimeRange, windowParam } from '@/shared/components/time-range';
import { VirtualList } from '@/shared/components/virtual-list';
import { cn, formatDuration, formatTime, pinchId } from '@/shared/lib/utils';

const routeApi = getRouteApi('/evaluation');

const REPORT_PAGE = 50;

/**
 * Everything, until somebody narrows it.
 *
 * Every other list here defaults to a day, because everything on them goes
 * when the log's retention takes it. Half of this one is kept on purpose for
 * thirty days *because* it outlives that log, so a day would hide the evidence
 * this screen exists to show. The control narrows the view; it does not define
 * it.
 */
const DEFAULT_EVALUATION_WINDOW = 0;

/** When a row happened, in the one unit the two halves have in common. */
function timeOf(row: Row): number {
  return row.kind === 'evidence'
    ? row.item.receipt.committed_at
    : Date.parse(row.item.started_at) / 1000;
}

/**
 * The oldest row a merge of two paged lists may show.
 *
 * A list that still has pages can deliver a row that belongs above one already
 * drawn, so anything older than its tail is held back until it does. Without
 * this the list reorders under the reader every time a page arrives, which is
 * worse than a short list.
 */
function boundaryOf(rows: Row[], hasNextPage: boolean): number {
  const tail = rows.at(-1);
  return hasNextPage && tail ? timeOf(tail) : Number.NEGATIVE_INFINITY;
}

/** Both halves as one list, newest first, as far as both have been read. */
export function mergeRows(
  kept: Row[],
  folded: Row[],
  moreKept: boolean,
  moreFolded: boolean,
): Row[] {
  const merged = [...kept, ...folded].sort((a, b) => timeOf(b) - timeOf(a));
  const boundary = Math.max(boundaryOf(kept, moreKept), boundaryOf(folded, moreFolded));
  return Number.isFinite(boundary) ? merged.filter((row) => timeOf(row) >= boundary) : merged;
}

/** The two halves as rows, so nothing downstream re-wraps them. */
function rowsOf<T>(kind: 'evidence' | 'report', items: T[]): Row[] {
  return items.map((item) => ({ kind, item }) as Row);
}

/**
 * One list, two provenances.
 *
 * `evidence` is kept on purpose and outlives the traces behind it; `report` is
 * a fold of the event log and goes when that log's retention takes it. What you
 * can do with them differs, so the row says which it is rather than leaving
 * somebody to find out by clicking.
 */
export type Row =
  { kind: 'evidence'; item: DurableEvaluation } | { kind: 'report'; item: EvaluationSummary };

function idOf(row: Row): string {
  return row.kind === 'evidence' ? row.item.receipt.evaluation_id : row.item.evaluation_id;
}

export function EvaluationPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const [baselineDraft, setBaselineDraft] = React.useState(search.baseline ?? '');
  React.useEffect(() => setBaselineDraft(search.baseline ?? ''), [search.baseline]);

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

  // Kept evidence and folded reports are one question — "which measurements do
  // we have" — so they are one list, and every row says which it is. The two
  // sets never overlap: the API excludes a committed registry ID from the
  // legacy listing rather than reporting it twice.
  const window = search.window ?? DEFAULT_EVALUATION_WINDOW;
  const evidence = useEvidence(windowParam(window));

  const reports = useInfiniteQuery({
    queryKey: ['evaluations', search.suite, search.dataset, search.status, search.q, window],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listEvaluations({
        query: {
          window_seconds: windowParam(window),
          suite: search.suite,
          dataset: search.dataset,
          status: search.status,
          search: search.q || undefined,
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

  const foldedRows = React.useMemo(
    () => rowsOf('report', reports.data?.pages.flatMap((page) => page.evaluations) ?? []),
    [reports.data],
  );
  const keptRows = React.useMemo(
    () => rowsOf('evidence', evidence.data?.pages.flatMap((page) => page.evaluations) ?? []),
    [evidence.data],
  );
  // Newest first, across both halves. The catalogue has a published order now
  // (ADR_0030), so interleaving states a fact rather than inventing one — and
  // provenance stays on every row, because what a row *is* does not follow
  // from where it sits.
  const rows: Row[] = React.useMemo(
    () => mergeRows(keptRows, foldedRows, evidence.hasNextPage, reports.hasNextPage),
    [keptRows, foldedRows, evidence.hasNextPage, reports.hasNextPage],
  );
  const total = reports.data?.pages[0]?.total_known ?? 0;
  const retention = evidence.data?.pages[0]?.retention;
  // What the last collection pass found missing, by ID. A summary reads one
  // object and cannot see this, so the row carries it or nobody learns it
  // without opening every result.
  const damaged = React.useMemo(() => new Set(retention?.damaged ?? []), [retention]);
  const evidenceFailure = evidence.error instanceof ApiFailure ? evidence.error : undefined;

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
        <div className="flex items-center gap-2">
          <TimeRange value={window} onChange={(seconds) => select({ window: seconds })} />
          <Button
            size="sm"
            variant={search.approvals ? 'default' : 'outline'}
            aria-pressed={search.approvals === true}
            onClick={() => select({ approvals: search.approvals ? undefined : true })}
          >
            Approvals
          </Button>
        </div>
      </div>
      {search.approvals ? <Approvals /> : null}

      <LocalViews
        screen="evaluation"
        search={search}
        onRestore={(value) => void navigate({ search: searchSchema.parse(value) })}
      />
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

      <form
        className="flex flex-wrap items-end gap-2 text-xs"
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          select({ baseline: String(data.get('baseline') ?? '').trim() || undefined });
        }}
      >
        <label>
          Baseline ID (blank = previous success)
          <input
            name="baseline"
            aria-label="Baseline ID"
            list="baseline-options"
            value={baselineDraft}
            onChange={(event) => setBaselineDraft(event.target.value)}
            className="ml-2 rounded border border-border bg-background p-2"
          />
        </label>
        <datalist id="baseline-options">
          {foldedRows
            .map((row) => row.item as EvaluationSummary)
            .filter((report) => report.evaluation_id !== search.report)
            .map((report) => (
              <option value={report.evaluation_id} key={report.evaluation_id}>
                {report.status} · {report.suite}
              </option>
            ))}
        </datalist>
        <Button type="submit" size="sm">
          Apply baseline
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => select({ baseline: undefined })}
        >
          Automatic baseline
        </Button>
        <span className="text-muted-foreground">
          Suggestions use loaded reports; paste any retained report ID.
        </span>
      </form>
      <Filters
        search={search}
        onSelect={select}
        summary={reports.isLoading ? 'loading…' : `${total} report${total === 1 ? '' : 's'}`}
      />

      <div className="grid gap-4 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
        <div className="flex min-w-0 flex-col gap-2">
          <Card className="overflow-hidden">
            {reports.isError ? (
              <EmptyState
                title="Could not reach the API"
                hint="Is the aiwatcher server running? The panel proxies /api to it in development."
              />
            ) : rows.length === 0 && !reports.isLoading && !evidence.isLoading ? (
              <EmptyState
                title="No evaluation reports and no kept evidence"
                hint="Publish an eval.completed event — the Python SDK's record_evaluation is one call — or publish durable evidence through /api/v1/evaluation-results."
              />
            ) : (
              <VirtualList
                items={rows}
                className="max-h-[34rem]"
                estimateSize={62}
                keyOf={(row) => `${row.kind}:${idOf(row)}`}
                onReachEnd={() => {
                  // Whichever half the boundary is waiting on. Fetching the
                  // other one adds rows the merge would hold back anyway.
                  const kept = boundaryOf(keptRows, evidence.hasNextPage);
                  const folded = boundaryOf(foldedRows, reports.hasNextPage);
                  if (kept >= folded && evidence.hasNextPage && !evidence.isFetchingNextPage) {
                    void evidence.fetchNextPage();
                  } else if (reports.hasNextPage && !reports.isFetchingNextPage) {
                    void reports.fetchNextPage();
                  }
                }}
                isFetchingMore={reports.isFetchingNextPage || evidence.isFetchingNextPage}
                renderRow={(row) =>
                  row.kind === 'evidence' ? (
                    <EvidenceRow
                      evidence={row.item}
                      gaps={damaged.has(idOf(row))}
                      selected={idOf(row) === search.evidence}
                      onSelect={() => select({ evidence: idOf(row), report: undefined })}
                    />
                  ) : (
                    <ReportRow
                      report={row.item}
                      selected={idOf(row) === search.report}
                      onSelect={() => select({ report: idOf(row), evidence: undefined })}
                    />
                  )
                }
              />
            )}
          </Card>
          {evidenceFailure ? (
            <Card>
              <EvidenceUnavailable failure={evidenceFailure} />
            </Card>
          ) : (
            <Retention report={retention} loading={evidence.isLoading} />
          )}
        </div>

        {search.evidence ? (
          <EvidencePane
            evaluationId={search.evidence}
            gaps={damaged.has(search.evidence) ? (retention ?? undefined) : undefined}
          />
        ) : (
          <ReportPane
            evaluationId={search.report}
            baselineId={search.baseline}
            selectedMetrics={search.metrics?.split(',')}
          />
        )}
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
          aria-label="Search evaluations"
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

function ReportPane({
  evaluationId,
  baselineId,
  selectedMetrics,
}: {
  evaluationId?: string;
  baselineId?: string;
  selectedMetrics?: string[];
}) {
  const detail = useQuery({
    queryKey: ['evaluation', evaluationId, baselineId],
    enabled: Boolean(evaluationId),
    queryFn: async () => {
      const response = await getEvaluation({
        throwOnError: true,
        path: { evaluation_id: evaluationId! },
        query: { baseline_id: baselineId },
      });
      if (!response.data) throw new Error('failed to load the evaluation');
      return response.data;
    },
    retry: false,
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
          title="Could not load the report or requested baseline"
          hint="Check the IDs or retry. Reports may have expired from the bounded projection. No replacement baseline was selected."
        />
      </Card>
    );
  }

  return <ReportDetail detail={detail.data} selectedMetrics={selectedMetrics} />;
}

export function ReportDetail({
  detail,
  selectedMetrics,
}: {
  detail: EvaluationDetail;
  selectedMetrics?: string[];
}) {
  const navigate = routeApi.useNavigate();
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
              {summary.dataset && (
                <DatasetReference
                  reference={summary.dataset}
                  kind={summary.context?.dataset_kind}
                  version={summary.context?.dataset_version}
                />
              )}
              {summary.execution_id && (
                <ExecutionReference executionId={summary.execution_id} stepId={summary.step_id} />
              )}
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

      <Card className="p-4 text-xs">
        <h3 className="text-sm font-semibold">Comparison evidence</h3>
        {comparison ? (
          <>
            <ComparabilityControl value={comparison.comparability} />
            <p className="mt-2">
              baseline{' '}
              <IdChip
                value={pinchId(comparison.baseline_id, 10, 8)}
                full={comparison.baseline_id}
              />
            </p>
            <ul className="mt-1 list-disc pl-4">
              {comparison.reasons.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
            <p>
              Common retained cases: {comparison.common_cases}. Current:{' '}
              {comparison.current_cases_retained}/{summary.cases_total}; baseline:{' '}
              {comparison.baseline_cases_retained}/{comparison.baseline_summary.cases_total}.
            </p>
            <p>
              {comparison.details_complete
                ? 'All reported details retained.'
                : 'Partial details — this is not a complete regression analysis.'}
            </p>
          </>
        ) : (
          <p>
            No earlier successful baseline on this suite and dataset. Select a baseline explicitly
            to inspect another report.
          </p>
        )}
        <p>
          Current context:{' '}
          {Object.entries(summary.context ?? {})
            .map(([key, value]) => `${key}: ${value ?? 'unknown'}`)
            .join(' · ')}
        </p>
        {comparison && (
          <p>
            Baseline data: {comparison.baseline_summary.dataset ?? 'unknown'} ·{' '}
            {Object.entries(comparison.baseline_summary.context ?? {})
              .map(([key, value]) => `${key}: ${value ?? 'unknown'}`)
              .join(' · ')}
          </p>
        )}
        <p>
          Retained cases: {detail.cases.length}/{summary.cases_total}. Report:{' '}
          {summary.report_dropped
            ? 'detail lost'
            : summary.report_bytes
              ? 'retained'
              : 'not supplied'}
          .
        </p>
      </Card>
      <fieldset className="flex flex-wrap gap-3 text-xs">
        <legend>Metrics</legend>
        {[
          ...new Set([
            ...Object.keys(summary.metrics),
            ...(comparison?.metrics.map((metric) => metric.name) ?? []),
          ]),
        ].map((name) => (
          <label key={name}>
            <input
              type="checkbox"
              checked={!selectedMetrics?.length || selectedMetrics.includes(name)}
              onChange={() => {
                const all =
                  comparison?.metrics.map((metric) => metric.name) ?? Object.keys(summary.metrics);
                const shown = selectedMetrics?.length ? selectedMetrics : all;
                const next =
                  shown.includes(name) && shown.length > 1
                    ? shown.filter((entry) => entry !== name)
                    : [...new Set([...shown, name])];
                void navigate({ search: (previous) => ({ ...previous, metrics: next.join(',') }) });
              }}
            />{' '}
            {name}
          </label>
        ))}
      </fieldset>
      <div className="grid min-w-0 gap-4">
        <Metrics
          metrics={Object.fromEntries(
            Object.entries(summary.metrics).filter(
              ([name]) => !selectedMetrics?.length || selectedMetrics.includes(name),
            ),
          )}
          comparison={comparison?.metrics.filter(
            (metric) => !selectedMetrics?.length || selectedMetrics.includes(metric.name),
          )}
          baselineId={comparison?.baseline_id}
          withheld={Boolean(comparison) && comparison?.comparability !== 'comparable'}
        />
        <ParameterComparison
          entries={[
            { id: summary.evaluation_id, params: summary.params },
            ...(comparison
              ? [{ id: comparison.baseline_id, params: comparison.baseline_summary.params }]
              : []),
          ]}
        />
      </div>

      {comparison ? <Regressions detail={detail} /> : null}

      <Cases detail={detail} />

      <ReportDocument detail={detail} />
    </div>
  );
}

/**
 * Comparability, as a control rather than as a fourth sentence.
 *
 * The server distinguishes three states and decides which one this is (A3, and
 * ADR_0030's comparison rules): `comparable` means the deltas below mean
 * something, `unverified` means the evidence for that judgement is missing, and
 * `incompatible` means two facts are being compared that are not one fact. The
 * panel implements none of it — it renders which of the three the server chose,
 * and says plainly that a delta is withheld rather than leaving it absent.
 */
function ComparabilityControl({ value }: { value: Comparability }) {
  const SAYS: Record<Comparability, string> = {
    comparable: 'Deltas below are a like-for-like comparison.',
    unverified: 'Deltas are withheld: the evidence for a like-for-like comparison is missing.',
    incompatible:
      'Deltas are withheld: these two were not measured on the same thing, so a difference between them is not a change.',
  };
  return (
    <div role="group" aria-label="Comparability">
      <div className="flex flex-wrap items-center gap-1">
        {(['comparable', 'unverified', 'incompatible'] as const).map((state) => (
          <span
            key={state}
            aria-current={state === value ? 'true' : undefined}
            className={cn(
              'rounded-md border px-2 py-0.5',
              state === value
                ? state === 'comparable'
                  ? 'border-primary bg-primary/10 font-medium text-foreground'
                  : 'border-warning bg-warning/10 font-medium text-foreground'
                : 'border-border/60 text-muted-foreground/60',
            )}
          >
            {state}
          </span>
        ))}
      </div>
      <p className="mt-1">{SAYS[value]}</p>
    </div>
  );
}

function Metrics({
  metrics,
  comparison,
  baselineId,
  withheld,
}: {
  metrics: Record<string, number>;
  comparison?: MetricDelta[];
  baselineId?: string;
  /** The server refused to compute deltas. Drawn as withheld, never as absent. */
  withheld?: boolean;
}) {
  // With a baseline, the comparison already lists every metric either side
  // reported — including one that appeared or disappeared, which is exactly
  // what a plain map of this report's metrics would hide.
  const rows: MetricDelta[] =
    comparison ?? Object.entries(metrics).map(([name, value]) => ({ name, current: value }));

  return (
    <Card className="overflow-auto">
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
          <thead>
            <tr>
              <th scope="col" className="px-4 py-1.5">
                Metric
              </th>
              <th scope="col" className="px-4 py-1.5 text-right">
                Current
              </th>
              {baselineId && (
                <th scope="col" className="px-4 py-1.5 text-right">
                  Baseline
                </th>
              )}
              <th scope="col" className="px-4 py-1.5 text-right">
                Delta
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.name} className="border-t border-border/40">
                <td className="px-4 py-1.5 text-muted-foreground">{row.name}</td>
                <td className="px-4 py-1.5 text-right tabular-nums">
                  {row.current === undefined || row.current === null
                    ? '—'
                    : formatMetric(row.current)}
                </td>
                {baselineId && (
                  <td className="px-4 py-1.5 text-right tabular-nums">
                    {row.baseline == null ? '—' : formatMetric(row.baseline)}
                  </td>
                )}
                <td className="w-24 px-4 py-1.5 text-right text-xs tabular-nums">
                  {/* An empty cell reads as "no change". A withheld delta is a
                      decision the server made and has to look like one. */}
                  {withheld ? (
                    <span className="text-muted-foreground">withheld</span>
                  ) : (
                    <Delta value={row.delta} />
                  )}
                </td>
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
          Among retained common cases, against {comparison.baseline_id}, run{' '}
          {formatTime(comparison.baseline_started_at)}. A case that passed then and fails now is the
          one thing on this page whose direction is not a matter of interpretation.
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
