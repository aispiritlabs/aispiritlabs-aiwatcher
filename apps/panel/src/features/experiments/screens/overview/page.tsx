import { useQuery } from '@tanstack/react-query';
import { getRouteApi, Link } from '@tanstack/react-router';

import { getExperiment, listExperiments } from '@/api/generated/sdk.gen';
import type {
  EvidenceMetricDelta,
  ExecutionSummary,
  ExperimentEntry,
  ExperimentRow,
  LatencySummary,
  MetricDefinition,
  TokenCost,
  TokenSummary,
  VariantObservations,
} from '@/api/generated/types.gen';
import { TimeRange, windowParam } from '@/shared/components/time-range';
import { Badge, Card, EmptyState, IdChip, Spinner } from '@/shared/components/ui/primitives';
import { ApiFailure, answerOf } from '@/shared/lib/result';
import { cn, formatDuration, pinchId } from '@/shared/lib/utils';

/**
 * Experiments: the variants measured on one pinned context, side by side.
 *
 * Every number is the server's. Quality, sample size, failures and what the
 * answers took are the published evidence's, which outlives the log; how long
 * each whole run took is the log's own fold, for as long as it keeps the run —
 * two different clocks, drawn in two different columns. A row against the
 * baseline carries the server's answer about whether the two compare, and a
 * delta it withheld is drawn as withheld. Nothing is averaged across rows: a
 * variant measured twice is two rows, because a mean of two p90s is no p90.
 *
 * What a variant was observed doing is a third clock: the runs on the log that
 * named it and that no measurement made, over the window in the URL. It is the
 * server's fold of the log, one per variant rather than per row, and it is
 * never set against a result's numbers as though the two were one sample.
 */

const routeApi = getRouteApi('/experiments');

export function ExperimentsPage() {
  const search = routeApi.useSearch();
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Experiments</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          The variants measured on one pinned context — the same cases, split, card and scorer —
          with their quality, how many cases each covered, what the answers took and how long each
          run ran — and, beside them, what each variant was observed doing in runs that named it
          outside a measurement. No price source is configured, so tokens are counted and nothing is
          priced.
        </p>
      </div>
      {search.context ? (
        <OneExperiment context={search.context} baseline={search.baseline} window={search.window} />
      ) : (
        <Contexts />
      )}
    </div>
  );
}

function Contexts() {
  const navigate = routeApi.useNavigate();
  const index = useQuery({
    queryKey: ['experiments'],
    queryFn: async () => answerOf(await listExperiments(), 'could not read the experiments'),
    retry: false,
  });
  const failure = index.error instanceof ApiFailure ? index.error : undefined;
  if (failure) {
    return (
      <p className="text-sm text-danger">
        {failure.status === 501
          ? 'This instance keeps no durable evidence, so it has no experiments.'
          : `Could not read the experiments: ${failure.message}`}
      </p>
    );
  }
  if (index.isLoading) return <Spinner />;
  const experiments = index.data?.experiments ?? [];
  if (experiments.length === 0) {
    return (
      <EmptyState
        title="No published results yet"
        hint="A result measured on a pinned context shows up here, with every other variant measured on it."
      />
    );
  }
  return (
    <Card className="overflow-x-auto p-3 text-xs">
      <table className="w-full text-left">
        <thead className="text-muted-foreground">
          <tr>
            <th className="font-normal">Measured on</th>
            <th className="font-normal">Card</th>
            <th className="font-normal">Cases</th>
            <th className="font-normal">Variants</th>
            <th className="font-normal">Results</th>
            <th className="font-normal">Latest</th>
          </tr>
        </thead>
        <tbody>
          {experiments.map((entry) => (
            <tr key={entry.context_id} className="border-t border-border/40">
              <td className="py-1.5">
                <button
                  type="button"
                  className="text-left text-primary hover:underline"
                  onClick={() => void navigate({ search: { context: entry.context_id } })}
                >
                  {measuredOn(entry)}
                </button>
              </td>
              <td>
                {entry.suite ? `${entry.suite.name} @ ${pinchId(entry.suite.version, 6, 4)}` : '—'}
              </td>
              <td>{entry.case_count ?? '—'}</td>
              <td>{entry.variants}</td>
              <td>{entry.results}</td>
              <td>{new Date(entry.latest_committed_at * 1000).toLocaleString()}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {index.data?.truncated ? (
        <p className="mt-2 text-muted-foreground">
          Read from the newest thousand results; older experiments are not listed.
        </p>
      ) : null}
    </Card>
  );
}

function measuredOn(entry: ExperimentEntry): string {
  if (!entry.dataset) return pinchId(entry.context_id, 8, 6);
  return `${entry.dataset.kind} ${entry.dataset.name} @ ${pinchId(entry.dataset.version, 6, 4)}${
    entry.split ? `, ${entry.split}` : ''
  }`;
}

function OneExperiment({
  context,
  baseline,
  window,
}: {
  context: string;
  baseline?: string;
  window?: number;
}) {
  const navigate = routeApi.useNavigate();
  const read = useQuery({
    queryKey: ['experiment', context, baseline, window],
    queryFn: async () =>
      answerOf(
        await getExperiment({
          path: { context_id: context },
          query: { baseline, window_seconds: windowParam(window) },
        }),
        'could not read this experiment',
      ),
    retry: false,
  });
  const back = (
    <button
      type="button"
      className="self-start text-xs text-primary hover:underline"
      onClick={() => void navigate({ search: {} })}
    >
      ← Every experiment
    </button>
  );
  if (read.error) {
    return (
      <div className="flex flex-col gap-2">
        {back}
        <p className="text-sm text-danger">
          {read.error instanceof ApiFailure && read.error.status === 404
            ? baseline
              ? `No result named ${baseline} was published in this context.`
              : 'Nothing was published in this context.'
            : read.error instanceof Error
              ? read.error.message
              : String(read.error)}
        </p>
      </div>
    );
  }
  if (read.isLoading || !read.data) return <Spinner />;
  const { experiment, executions, observed } = read.data;
  const timing = new Map(executions.map((execution) => [execution.workflow_run_id, execution]));
  const observations = new Map(observed.map((variant) => [variant.variant_id, variant]));
  return (
    <div className="flex flex-col gap-3">
      {back}
      <Card className="flex flex-wrap items-center gap-2 p-3 text-xs">
        <span className="text-muted-foreground">Context</span>
        <IdChip label="context" value={pinchId(context, 8, 6)} full={context} />
        {baseline ? (
          <>
            <span className="text-muted-foreground">compared with</span>
            <Badge tone="primary">{baseline}</Badge>
            <button
              type="button"
              className="text-primary hover:underline"
              onClick={() => void navigate({ search: { context, window } })}
            >
              compare with nothing
            </button>
          </>
        ) : (
          <span className="text-muted-foreground">
            — choose a baseline to see each row&apos;s change.
          </span>
        )}
        <span className="ml-auto flex items-center gap-2">
          <span className="text-muted-foreground">observed in the</span>
          <TimeRange
            value={window ?? 0}
            onChange={(seconds) =>
              void navigate({ search: { context, baseline, window: seconds } })
            }
          />
        </span>
      </Card>
      <Card className="overflow-x-auto p-3 text-xs">
        <table className="w-full text-left">
          <thead className="text-muted-foreground">
            <tr>
              <th className="font-normal">Variant</th>
              <th className="font-normal">Result</th>
              <th className="font-normal">Cases</th>
              {experiment.metrics.map((metric) => (
                <th key={metric.name} className="font-normal" title={definition(metric)}>
                  {metric.name}
                </th>
              ))}
              <th
                className="font-normal"
                title="Per case, by nearest rank over the cases that reported it"
              >
                Answer latency p50 / p90 / p99
              </th>
              <th className="font-normal">Tokens in / out</th>
              <th
                className="font-normal"
                title="The whole managed run, every step and retry, as the log folded it"
              >
                Run
              </th>
              <th
                className="font-normal"
                title="Runs on the log that named this variant and that no measurement made, in the window — another sample, on the log's clock"
              >
                Observed
              </th>
              <th />
            </tr>
          </thead>
          <tbody>
            {experiment.rows.map((row) => (
              <Row
                key={row.evaluation_id}
                row={row}
                metrics={experiment.metrics}
                isBaseline={row.evaluation_id === baseline}
                execution={
                  row.origin?.execution_id ? timing.get(row.origin.execution_id) : undefined
                }
                observed={observations.get(row.variant_id)}
                onBaseline={() =>
                  void navigate({ search: { context, baseline: row.evaluation_id, window } })
                }
              />
            ))}
          </tbody>
        </table>
        {experiment.truncated ? (
          <p className="mt-2 text-muted-foreground">
            Only the newest thousand results in this context are shown.
          </p>
        ) : null}
      </Card>
    </div>
  );
}

function Row({
  row,
  metrics,
  isBaseline,
  execution,
  observed,
  onBaseline,
}: {
  row: ExperimentRow;
  metrics: MetricDefinition[];
  isBaseline: boolean;
  execution: ExecutionSummary | undefined;
  observed: VariantObservations | undefined;
  onBaseline: () => void;
}) {
  const counts = row.counts;
  const selected = counts?.selected ?? 0;
  const deltas = new Map((row.comparison?.metrics ?? []).map((metric) => [metric.name, metric]));
  const prompt = row.variant?.prompt;
  const model = row.variant?.model;
  return (
    <tr className={cn('border-t border-border/40 align-top', isBaseline && 'bg-primary/5')}>
      <td className="py-1.5">
        <div className="font-medium">
          {row.variant?.experiment_id ?? pinchId(row.variant_id, 8, 6)}
        </div>
        <div className="text-muted-foreground">
          {[
            prompt ? `prompt ${prompt.name} @ ${pinchId(prompt.version, 6, 4)}` : null,
            model ? `model ${model.name} @ ${model.version}` : null,
            row.origin?.repetition_id,
          ]
            .filter(Boolean)
            .join(' · ')}
        </div>
      </td>
      <td>
        <Link
          to="/evaluation"
          search={{ evidence: row.evaluation_id }}
          className="text-primary hover:underline"
        >
          {row.evaluation_id}
        </Link>
        <div className="flex flex-wrap gap-1 pt-0.5">
          <Badge tone={row.state === 'complete' ? 'success' : 'warning'}>{row.state}</Badge>
          {row.reproducible ? null : <Badge tone="warning">a model&apos;s word</Badge>}
          {row.comparison ? (
            <Badge tone={row.comparison.comparability === 'comparable' ? 'neutral' : 'danger'}>
              {row.comparison.comparability}
            </Badge>
          ) : null}
        </div>
        {row.comparison && row.comparison.reasons.length > 0 ? (
          <div className="text-muted-foreground">{row.comparison.reasons.join('; ')}</div>
        ) : null}
      </td>
      <td>
        {counts ? (
          <>
            <div>{`${counts.scored} of ${selected} scored`}</div>
            <div className="text-muted-foreground">
              {`${counts.failed} failed · ${counts.unscored} unscored`}
            </div>
          </>
        ) : (
          '—'
        )}
      </td>
      {metrics.map((metric) => (
        <td key={metric.name}>
          <div>{value(row.metrics[metric.name])}</div>
          {row.comparison ? <Delta delta={deltas.get(metric.name)} /> : null}
        </td>
      ))}
      <td>
        <Latency latency={row.usage?.latency_ms} selected={selected} />
      </td>
      <td>
        <Tokens
          input={row.usage?.input_tokens}
          output={row.usage?.output_tokens}
          selected={selected}
        />
        {row.cost ? <Cost cost={row.cost} /> : null}
      </td>
      <td>
        {execution ? (
          <Link
            to="/workflows"
            search={{ workflow: execution.workflow_id, execution: execution.workflow_run_id }}
            className="text-primary hover:underline"
          >
            {execution.duration_ms === undefined || execution.duration_ms === null
              ? execution.status
              : formatDuration(execution.duration_ms)}
          </Link>
        ) : row.origin?.execution_id ? (
          <span className="text-muted-foreground" title="The log no longer holds this run">
            not in the log
          </span>
        ) : (
          <span className="text-muted-foreground">published, not run here</span>
        )}
      </td>
      <td>
        <Observed observed={observed} />
      </td>
      <td>
        {isBaseline ? (
          <Badge tone="primary">baseline</Badge>
        ) : (
          <button type="button" className="text-primary hover:underline" onClick={onBaseline}>
            Use as baseline
          </button>
        )}
      </td>
    </tr>
  );
}

/**
 * The runs that named a variant outside a measurement. A count of nothing is
 * drawn as nothing observed rather than as zeros, and the measurement's own
 * runs are said to be left out, so a benchmark never reads as traffic.
 *
 * What the calls cost is the server's, at the deployment's price table, and it
 * says when and where each price was read; calls no price covers are counted as
 * unpriced rather than as free. A window counts the runs that ended from its
 * start on, where they ended, so it says where counting began — the window's
 * start, or later where nothing was observed before — and how many runs reached
 * the log after their period had closed. Figures over periods are bucketed,
 * and say so.
 */
function Observed({ observed }: { observed: VariantObservations | undefined }) {
  if (!observed || observed.runs === 0) {
    return (
      <span className="text-muted-foreground">
        {observed && observed.measured_runs > 0
          ? `no runs outside a measurement (${observed.measured_runs} measured)`
          : 'no runs named it'}
      </span>
    );
  }
  const duration = observed.duration_ms;
  const calls = observed.call_ms;
  const first = observed.time_to_first_token_ms;
  const cost = observed.cost;
  const spread = (summary: { p50: number; p90: number; p99: number; bucketed?: boolean }) =>
    `${summary.bucketed ? '≤ ' : ''}${ms(summary.p50)} / ${ms(summary.p90)} / ${ms(summary.p99)}`;
  return (
    <>
      <Link
        to="/observability/explore"
        search={{ by: 'variant', key: observed.variant_id }}
        className="text-primary hover:underline"
      >
        {`${observed.runs} runs · ${observed.failed} failed`}
      </Link>
      {duration ? <div>{spread(duration)}</div> : null}
      {calls ? (
        <div>{`a call ${spread(calls)}${first ? ` · first token ${ms(first.p50)}` : ''}`}</div>
      ) : null}
      <div className="text-muted-foreground">
        {[
          duration ? `over ${duration.count} finished` : 'none finished',
          `${observed.input_tokens.toLocaleString()} / ${observed.output_tokens.toLocaleString()} tokens in ${observed.llm_calls} calls`,
          observed.measured_runs > 0 ? `${observed.measured_runs} measured runs left out` : null,
          observed.periods > 0
            ? `${observed.runs_from_periods} runs from ${observed.periods} written ${
                observed.periods === 1 ? 'period' : 'periods'
              }${observed.incomplete_periods > 0 ? `, ${observed.incomplete_periods} incomplete` : ''}`
            : null,
          observed.counted_from
            ? `counted from ${observed.counted_from.slice(0, 19).replace('T', ' ')} UTC${
                observed.window_before_observations ? ', nothing observed before' : ''
              }`
            : null,
          (observed.late_runs ?? 0) > 0
            ? `${observed.late_runs} reached the log after their period closed`
            : null,
          ...(observed.missed ?? []).map(
            (gap) =>
              `${gap.events} events from ${gap.from.slice(0, 19).replace('T', ' ')} to ${gap.until
                .slice(0, 19)
                .replace(
                  'T',
                  ' ',
                )} UTC never reached the fold, so runs that ended then may be missing`,
          ),
        ]
          .filter(Boolean)
          .join(' · ')}
      </div>
      {cost ? <Cost cost={cost} /> : null}
    </>
  );
}

function Delta({ delta }: { delta: EvidenceMetricDelta | undefined }) {
  if (!delta || delta.delta === undefined || delta.delta === null) {
    return <div className="text-muted-foreground">change withheld</div>;
  }
  const change = delta.delta;
  const better =
    change === 0 || !delta.direction || delta.direction === 'none'
      ? undefined
      : (delta.direction === 'higher') === change > 0;
  return (
    <div className={cn(better === true && 'text-primary', better === false && 'text-danger')}>
      {`${change > 0 ? '+' : ''}${change.toFixed(4)}`}
    </div>
  );
}

function Latency({ latency, selected }: { latency?: LatencySummary | null; selected: number }) {
  if (!latency) return <span className="text-muted-foreground">not measured</span>;
  return (
    <>
      <div>{`${ms(latency.p50)} / ${ms(latency.p90)} / ${ms(latency.p99)}`}</div>
      <div className="text-muted-foreground">{`over ${latency.cases} of ${selected} cases`}</div>
    </>
  );
}

function Tokens({
  input,
  output,
  selected,
}: {
  input?: TokenSummary | null;
  output?: TokenSummary | null;
  selected: number;
}) {
  if (!input && !output) return <span className="text-muted-foreground">not counted</span>;
  const side = (summary?: TokenSummary | null) => (summary ? summary.total.toLocaleString() : '—');
  const cases = Math.max(input?.cases ?? 0, output?.cases ?? 0);
  return (
    <>
      <div>{`${side(input)} / ${side(output)}`}</div>
      <div className="text-muted-foreground">{`counted on ${cases} of ${selected} cases`}</div>
    </>
  );
}

/**
 * What calls cost at the deployment's price table, as the server priced them —
 * each call at the price in force on its day: with the day each price used was
 * read for its model, the calls priced by a price read after they were made,
 * and the calls no price covered counted apart rather than as free.
 */
function Cost({ cost }: { cost: TokenCost }) {
  return (
    <div className="text-muted-foreground">
      {cost.priced_calls > 0
        ? `${cost.amount.toLocaleString(undefined, { maximumSignificantDigits: 3 })} ${cost.currency} for ${cost.priced_calls} priced calls, at ${cost.prices
            .map((price) => `${price.model} as of ${price.as_of}`)
            .join(', ')}`
        : 'no call was priced'}
      {cost.priced_before_read
        ? ` · ${cost.priced_before_read} made before any price for their model was read, priced by the earliest`
        : ''}
      {cost.unpriced_calls > 0
        ? ` · ${cost.unpriced_calls} calls unpriced (${(cost.unpriced_models ?? []).join(', ')})`
        : ''}
    </div>
  );
}

function definition(metric: MetricDefinition): string {
  return `${metric.unit}, ${metric.direction} is better, ${metric.aggregation}`;
}

function value(number: number | undefined): string {
  return number === undefined ? '—' : Number.isInteger(number) ? String(number) : number.toFixed(4);
}

function ms(value: number): string {
  return formatDuration(value);
}
