/**
 * One published result against another, and who decides which two those are.
 *
 * The candidates are not filtered here. Which results may be compared is the
 * server's rule — every result sharing a `context_id` was measured on the same
 * cohort, split, suite, scorer and metric definitions, by content — so the list
 * comes from `GET /api/v1/evaluation-results?context_id=…` and this renders
 * what came back. A browser deciding it would be a second answer to what a
 * comparison is, in a language that cannot see the contexts.
 *
 * There is no automatic baseline. The folded half has one because a log fold
 * has no other way to offer a pair; here the catalogue answers that, and which
 * of the candidates is the baseline is somebody's decision rather than a
 * default they might not notice.
 */
import { Fragment, useState } from 'react';
import { useInfiniteQuery, useQueries, useQuery } from '@tanstack/react-query';

import { compareCases, compareResults, getCases, listResults } from '@/api/generated/sdk.gen';
import type {
  CaseChange,
  CaseOutcome,
  DurableEvaluation,
  EvidenceCaseDelta,
  EvidenceComparison,
  EvidenceMetricDelta,
} from '@/api/generated/types.gen';
import { Badge, Card, EmptyState, IdChip, Spinner } from '@/shared/components/ui/primitives';
import { answerOf } from '@/shared/lib/result';
import { cn, pinchId } from '@/shared/lib/utils';

import { ComparabilityControl } from './comparability';

const CANDIDATES = 50;
const DIFF_PAGE = 100;

/**
 * What the URL holds, which is one value more than the server's filter.
 *
 * Absent means the diff is closed, so "open it and filter nothing out" needs a
 * word of its own — `all`, which sends no `only` at all.
 */
export type CaseFilterChoice = 'worse' | 'better' | 'changed' | 'all';

const CHOICES: { value: CaseFilterChoice; label: string }[] = [
  { value: 'worse', label: 'Lost something' },
  { value: 'better', label: 'Gained something' },
  { value: 'changed', label: 'Moved at all' },
  { value: 'all', label: 'Every case' },
];

function nameOf(evidence: DurableEvaluation): string {
  return evidence.manifest?.variant.experiment_id ?? evidence.receipt.evaluation_id;
}

export function Comparison({
  evidence,
  baseline,
  onSelect,
  cases,
  onCases,
}: {
  evidence: DurableEvaluation;
  baseline: string | undefined;
  onSelect: (baseline: string | undefined) => void;
  cases?: CaseFilterChoice | undefined;
  onCases?: ((cases: CaseFilterChoice | undefined) => void) | undefined;
}) {
  const candidates = useQuery({
    queryKey: ['evaluation-comparable', evidence.receipt.context_id],
    queryFn: async () =>
      answerOf(
        await listResults({
          query: { context_id: evidence.receipt.context_id, limit: CANDIDATES },
        }),
        'could not read what this result may be compared with',
      ),
    retry: false,
  });
  const offered = (candidates.data?.evaluations ?? []).filter(
    (row) => row.receipt.evaluation_id !== evidence.receipt.evaluation_id,
  );
  // A pasted link may name a baseline this context does not offer, and that is
  // a question with an answer — "incompatible", and which field moved. The
  // control keeps showing what the URL asked for rather than resetting to the
  // candidates, which would quietly change what the reader is looking at.
  const chosen = baseline && !offered.some((row) => row.receipt.evaluation_id === baseline);

  return (
    <Card className="p-4 text-xs">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-semibold">Compared with</h3>
        <label className="flex items-center gap-2">
          <span className="sr-only">Baseline</span>
          <select
            aria-label="Baseline"
            className="rounded border border-border bg-background p-1"
            value={baseline ?? ''}
            onChange={(event) => onSelect(event.target.value || undefined)}
          >
            <option value="">Choose a baseline…</option>
            {chosen ? <option value={baseline}>{baseline}</option> : null}
            {offered.map((row) => (
              <option key={row.receipt.evaluation_id} value={row.receipt.evaluation_id}>
                {nameOf(row)} · {row.receipt.evaluation_id}
              </option>
            ))}
          </select>
        </label>
      </div>
      {baseline ? (
        <Comparand
          id={evidence.receipt.evaluation_id}
          baseline={baseline}
          cases={cases}
          onCases={onCases}
        />
      ) : candidates.isLoading ? (
        <Spinner />
      ) : offered.length === 0 ? (
        <p className="mt-2 text-muted-foreground">
          Nothing else has been published under this pinned context. A comparison needs a second
          result measured the same way — the same cases, split, suite, scorer and metric definitions
          — which is what the context ID above is the address of.
        </p>
      ) : (
        <p className="mt-2 text-muted-foreground">
          {offered.length} other result{offered.length === 1 ? '' : 's'} share this context and may
          be compared with this one.
        </p>
      )}
    </Card>
  );
}

function Comparand({
  id,
  baseline,
  cases,
  onCases,
}: {
  id: string;
  baseline: string;
  cases?: CaseFilterChoice | undefined;
  onCases?: ((cases: CaseFilterChoice | undefined) => void) | undefined;
}) {
  const comparison = useQuery({
    queryKey: ['evaluation-comparison', id, baseline],
    queryFn: async () =>
      answerOf(
        await compareResults({ path: { evaluation_id: id }, query: { baseline } }),
        'could not compare these two results',
      ),
    retry: false,
  });
  if (comparison.isLoading) return <Spinner />;
  if (!comparison.data) {
    return (
      <EmptyState
        title="This comparison could not be read"
        hint="One of the two results is no longer in the catalogue."
      />
    );
  }
  return <Verdict comparison={comparison.data} cases={cases} onCases={onCases} />;
}

function Verdict({
  comparison,
  cases,
  onCases,
}: {
  comparison: EvidenceComparison;
  cases?: CaseFilterChoice | undefined;
  onCases?: ((cases: CaseFilterChoice | undefined) => void) | undefined;
}) {
  return (
    <div className="mt-2 flex flex-col gap-2">
      <ComparabilityControl value={comparison.comparability} />
      <p>
        baseline{' '}
        <IdChip
          value={pinchId(comparison.baseline.receipt.evaluation_id, 10, 8)}
          full={comparison.baseline.receipt.evaluation_id}
        />{' '}
        {nameOf(comparison.baseline)}
      </p>
      {comparison.reasons.length > 0 ? (
        <ul className="list-disc pl-4">
          {comparison.reasons.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      ) : null}
      {/* Comparable and still not the question somebody usually means: one
          variant measured twice says how much this measurement moves on its
          own, which is worth knowing and is not an effect of a change. Only
          where there is a delta to misread — under a refusal the reasons have
          already said what is wrong, and this would describe a number that is
          not on screen. */}
      {comparison.same_variant && comparison.comparability === 'comparable' ? (
        <p className="rounded-md bg-muted/60 px-3 py-2">
          Both sides pin the same variant. The difference below is repetition — how much this
          measurement moves when nothing changed — rather than the effect of a change.
        </p>
      ) : null}
      <Deltas metrics={comparison.metrics} />
      {/* Stated rather than left to be discovered: the two headers are what a
          comparison costs, and the cases behind them are two full reads —
          which is why the next section is opened rather than fetched. */}
      <p className="text-muted-foreground">
        Metrics come from each result's own header, whatever number of cases is behind them.
      </p>
      {onCases && comparison.comparability !== 'incompatible' ? (
        <CaseDiff
          current={comparison.current}
          baseline={comparison.baseline}
          only={cases}
          onSelect={onCases}
        />
      ) : null}
    </div>
  );
}

/**
 * Which cases moved, behind a click.
 *
 * The comparison above reads two headers and costs what two summaries cost;
 * this reads both results in full, so it is not fetched because somebody opened
 * a result — it is fetched because they asked. The filter is the server's and
 * lives in the URL, so a link to "the cases that lost something" lands the next
 * reader on the same question rather than on a screen they have to redrive.
 *
 * Nothing is classified here. Whether a case regressed is a question about the
 * directions the pinned context declared, answered where those declarations
 * are; a browser deciding it from the sign of a delta would be guessing at
 * exactly the thing the context exists to state.
 */
function CaseDiff({
  current,
  baseline,
  only,
  onSelect,
}: {
  current: DurableEvaluation;
  baseline: DurableEvaluation;
  only: CaseFilterChoice | undefined;
  onSelect: (only: CaseFilterChoice | undefined) => void;
}) {
  return (
    <div className="mt-1 border-t border-border/60 pt-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium">Which cases moved</span>
        {CHOICES.map((choice) => (
          <button
            key={choice.value}
            type="button"
            className={cn(
              'rounded-full border px-2 py-0.5',
              only === choice.value
                ? 'border-primary bg-primary/10 text-primary'
                : 'border-border text-muted-foreground',
            )}
            onClick={() => onSelect(only === choice.value ? undefined : choice.value)}
          >
            {choice.label}
          </button>
        ))}
      </div>
      {only === undefined ? (
        <p className="mt-2 text-muted-foreground">
          Reads both results in full, a page at a time. Everything above is two headers.
        </p>
      ) : (
        <MovedCases current={current} baseline={baseline} only={only} />
      )}
    </div>
  );
}

function MovedCases({
  current,
  baseline,
  only,
}: {
  current: DurableEvaluation;
  baseline: DurableEvaluation;
  only: CaseFilterChoice;
}) {
  const id = current.receipt.evaluation_id;
  // Which row is open is a disclosure rather than a filter, so it stays here:
  // what the URL carries is the question somebody asked, and this is one row
  // of the answer they are already looking at.
  const [open, setOpen] = useState<string | undefined>(undefined);
  const diff = useInfiniteQuery({
    queryKey: ['evaluation-case-diff', id, baseline.receipt.evaluation_id, only],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      answerOf(
        await compareCases({
          path: { evaluation_id: id },
          query: {
            baseline: baseline.receipt.evaluation_id,
            cursor: pageParam,
            limit: DIFF_PAGE,
            ...(only === 'all' ? {} : { only }),
          },
        }),
        'could not read which cases moved',
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry: false,
  });

  if (diff.isLoading) return <Spinner />;
  const pages = diff.data?.pages ?? [];
  const rows = pages.flatMap((page) => page.cases);
  const damaged = pages.find((page) => page.state !== 'complete' && page.state !== 'partial');
  if (pages.length === 0) {
    return (
      <EmptyState
        title="These cases could not be read"
        hint="One of the two results is no longer in the catalogue."
      />
    );
  }
  return (
    <div className="mt-2 flex flex-col gap-2">
      {damaged ? (
        <p className="text-danger">
          The evidence behind this page is {damaged.state.replace(/_/g, ' ')}, so no rows are shown
          rather than some of them.
        </p>
      ) : null}
      {rows.length === 0 ? (
        <p className="text-muted-foreground">
          No case matched on the pages read so far
          {diff.hasNextPage ? ', and there are more to read' : ''}.
        </p>
      ) : (
        <table className="w-full">
          <thead className="text-muted-foreground">
            <tr>
              <th className="text-left font-normal">Case</th>
              <th className="text-left font-normal">Change</th>
              <th className="text-left font-normal">Metrics</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <Fragment key={row.case_id}>
                <tr className="border-t border-border/40 align-top">
                  <td className="py-1 pr-2 font-mono">
                    <button
                      type="button"
                      className="underline decoration-dotted underline-offset-2"
                      aria-expanded={open === row.case_id}
                      onClick={() => setOpen(open === row.case_id ? undefined : row.case_id)}
                    >
                      {row.case_id}
                    </button>
                  </td>
                  <td className="py-1 pr-2">
                    <ChangeBadge change={row.change} />
                  </td>
                  <td className="py-1">
                    <CaseMetrics row={row} />
                  </td>
                </tr>
                {open === row.case_id ? (
                  <tr className="border-t border-border/20">
                    <td colSpan={3} className="py-1">
                      <CaseAnswers row={row} current={current} baseline={baseline} />
                    </td>
                  </tr>
                ) : null}
              </Fragment>
            ))}
          </tbody>
        </table>
      )}
      {/* A page of a narrowed diff ends at the rows asked for or at the cases
          it was allowed to walk, so "more" here means more cases rather than
          more matches — the same promise the catalogue narrowed by context
          makes, for the same reason. */}
      {diff.hasNextPage ? (
        <button
          type="button"
          className="self-start text-primary underline"
          onClick={() => void diff.fetchNextPage()}
          disabled={diff.isFetchingNextPage}
        >
          {diff.isFetchingNextPage ? 'reading…' : 'read further into both results'}
        </button>
      ) : null}
    </div>
  );
}

/**
 * What each side actually answered, read from the route that holds it.
 *
 * The diff row carries no content — it carries *where* its case is on each
 * side, as a cursor the case route issued — so opening a row is two reads of
 * one case rather than a diff that shipped both results' answers to everybody
 * who only wanted to know which cases moved. Nothing here builds that cursor:
 * it is handed back exactly as it arrived.
 */
function CaseAnswers({
  row,
  current,
  baseline,
}: {
  row: EvidenceCaseDelta;
  current: DurableEvaluation;
  baseline: DurableEvaluation;
}) {
  const sides: { label: string; evidence: DurableEvaluation; outcome: CaseOutcome | undefined }[] =
    [
      { label: 'This result', evidence: current, outcome: row.current ?? undefined },
      { label: 'Baseline', evidence: baseline, outcome: row.baseline ?? undefined },
    ];
  const answers = useQueries({
    queries: sides.map((side) => ({
      queryKey: ['evaluation-case', side.evidence.receipt.evaluation_id, row.case_id],
      queryFn: async () =>
        answerOf(
          await getCases({
            path: { evaluation_id: side.evidence.receipt.evaluation_id },
            query: {
              version: side.evidence.receipt.version,
              cursor: side.outcome?.at,
              limit: 1,
            },
          }),
          'could not read this case',
        ),
      enabled: side.outcome !== undefined,
      retry: false,
    })),
  });

  return (
    <div className="grid gap-2 rounded-md bg-muted/40 px-3 py-2 sm:grid-cols-2">
      {sides.map((side, index) => {
        const answer = answers[index];
        const found = answer?.data?.cases[0];
        return (
          <div key={side.label} className="min-w-0">
            <p className="font-medium">{side.label}</p>
            {side.outcome === undefined ? (
              <p className="text-muted-foreground">never measured this case</p>
            ) : answer?.isLoading ? (
              <Spinner />
            ) : !found ? (
              <p className="text-muted-foreground">its evidence could not be read</p>
            ) : (
              <dl className="mt-0.5">
                <dt className="text-muted-foreground">expected</dt>
                <dd className="break-words">{JSON.stringify(found.expected)}</dd>
                <dt className="mt-1 text-muted-foreground">answered</dt>
                <dd className="break-words">
                  {found.measurement.error ? (
                    <span className="text-danger">{found.measurement.error}</span>
                  ) : (
                    JSON.stringify(found.measurement.actual)
                  )}
                </dd>
              </dl>
            )}
          </div>
        );
      })}
    </div>
  );
}

const CHANGES: Record<
  CaseChange,
  { label: string; tone: 'danger' | 'success' | 'warning' | 'neutral' }
> = {
  regressed: { label: 'regressed', tone: 'danger' },
  improved: { label: 'improved', tone: 'success' },
  mixed: { label: 'mixed', tone: 'warning' },
  unchanged: { label: 'unchanged', tone: 'neutral' },
  unmeasured: { label: 'unmeasured', tone: 'neutral' },
};

function ChangeBadge({ change }: { change: CaseChange }) {
  const shown = CHANGES[change];
  return <Badge tone={shown.tone}>{shown.label}</Badge>;
}

/**
 * One case's numbers, and the sentence a number cannot carry.
 *
 * A case that failed reports no score at all, so its row has one side of a
 * metric and no delta — which is the movement worth reading, and the reason the
 * error is shown beside the numbers rather than instead of them.
 */
function CaseMetrics({ row }: { row: EvidenceCaseDelta }) {
  const failed = row.current?.error;
  const was = row.baseline?.error;
  return (
    <div className="flex flex-col gap-0.5">
      {row.metrics.map((metric) => (
        <div key={metric.name} className="tabular-nums">
          <span className="text-muted-foreground">{metric.name}</span> {number(metric.current)}{' '}
          <span className="text-muted-foreground">was</span> {number(metric.baseline)}{' '}
          <span className={tone(metric)}>
            {metric.delta === undefined || metric.delta === null
              ? ''
              : `${metric.delta > 0 ? '+' : ''}${metric.delta.toFixed(4)}`}
          </span>
        </div>
      ))}
      {failed ? <p className="text-danger">{failed}</p> : null}
      {was && !failed ? <p className="text-muted-foreground">previously: {was}</p> : null}
      {row.change === 'unmeasured' ? (
        <p className="text-muted-foreground">
          {row.current ? 'the baseline' : 'this result'} never measured this case
        </p>
      ) : null}
    </div>
  );
}

/**
 * The one place in this area where a delta may be coloured.
 *
 * Everywhere else the direction of improvement is unknown — a producer's metric
 * may be a pass rate or a bill — so a green number could mean "we got more
 * expensive". A pinned context *declares* it, per metric, so a delta is only
 * coloured where `direction` says which way is better and stays plain where it
 * says `none` or where nobody declared the metric at all.
 */
function Deltas({ metrics }: { metrics: EvidenceMetricDelta[] }) {
  if (metrics.length === 0) return <p className="text-muted-foreground">No metrics reported.</p>;
  return (
    <table className="w-full table-fixed">
      <thead className="text-muted-foreground">
        <tr>
          <th className="w-2/5 text-left font-normal">Metric</th>
          <th className="text-right font-normal">This</th>
          <th className="text-right font-normal">Baseline</th>
          <th className="text-right font-normal">Delta</th>
        </tr>
      </thead>
      <tbody>
        {metrics.map((metric) => (
          <tr key={metric.name}>
            <td className="truncate">
              {metric.name}
              {metric.unit ? <span className="text-muted-foreground"> ({metric.unit})</span> : null}
            </td>
            <td className="text-right tabular-nums">{number(metric.current)}</td>
            <td className="text-right tabular-nums">{number(metric.baseline)}</td>
            <td className={cn('text-right tabular-nums', tone(metric))}>
              {metric.delta === undefined || metric.delta === null
                ? '—'
                : `${metric.delta > 0 ? '+' : ''}${metric.delta.toFixed(4)}`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function number(value: number | null | undefined): string {
  return value === undefined || value === null ? '—' : value.toFixed(4);
}

function tone(metric: EvidenceMetricDelta): string | undefined {
  const delta = metric.delta;
  if (delta === undefined || delta === null || delta === 0) return undefined;
  if (metric.direction === 'higher') return delta > 0 ? 'text-primary' : 'text-danger';
  if (metric.direction === 'lower') return delta < 0 ? 'text-primary' : 'text-danger';
  return undefined;
}
