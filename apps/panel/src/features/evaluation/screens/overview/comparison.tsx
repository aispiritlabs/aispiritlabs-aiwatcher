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
import { useQuery } from '@tanstack/react-query';

import { compareResults, listResults } from '@/api/generated/sdk.gen';
import type {
  DurableEvaluation,
  EvidenceComparison,
  EvidenceMetricDelta,
} from '@/api/generated/types.gen';
import { Card, EmptyState, IdChip, Spinner } from '@/shared/components/ui/primitives';
import { answerOf } from '@/shared/lib/result';
import { cn, pinchId } from '@/shared/lib/utils';

import { ComparabilityControl } from './comparability';

const CANDIDATES = 50;

function nameOf(evidence: DurableEvaluation): string {
  return evidence.manifest?.variant.experiment_id ?? evidence.receipt.evaluation_id;
}

export function Comparison({
  evidence,
  baseline,
  onSelect,
}: {
  evidence: DurableEvaluation;
  baseline: string | undefined;
  onSelect: (baseline: string | undefined) => void;
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
        <Comparand id={evidence.receipt.evaluation_id} baseline={baseline} />
      ) : candidates.isLoading ? (
        <Spinner />
      ) : offered.length === 0 ? (
        <p className="mt-2 text-muted-foreground">
          Nothing else has been published under this pinned context. A comparison needs a second
          result measured the same way — the same cases, split, suite, scorer and metric
          definitions — which is what the context ID above is the address of.
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

function Comparand({ id, baseline }: { id: string; baseline: string }) {
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
  return <Verdict comparison={comparison.data} />;
}

function Verdict({ comparison }: { comparison: EvidenceComparison }) {
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
          comparison costs, and the cases behind them are two full reads. */}
      <p className="text-muted-foreground">
        Metrics come from each result's own header. Which cases regressed is a read of both results
        in full and is not on this screen.
      </p>
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
              {metric.unit ? (
                <span className="text-muted-foreground"> ({metric.unit})</span>
              ) : null}
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
