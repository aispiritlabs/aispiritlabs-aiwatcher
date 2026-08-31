import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { z } from 'zod';

import { getMetrics } from '@/api/generated/sdk.gen';
import type { MetricsSummary, Percentiles } from '@/api/generated/types.gen';
import { Card, CardContent, CardHeader, CardTitle, EmptyState } from '@/components/ui/primitives';
import { RankedBars, type RankedRow } from '@/components/charts/ranked-bars';
import { StackedBars } from '@/components/charts/stacked-bars';
import { SERIES } from '@/components/charts/primitives';
import type { SeriesDef } from '@/components/charts/primitives';
import {
  DEFAULT_WINDOW_SECONDS,
  TimeRange,
  windowParam,
  windowSearchSchema,
} from '@/components/time-range';
import { formatCount, formatDuration } from '@/lib/utils';

/**
 * Metrics over the runs the projector still holds.
 *
 * Served from aiwatcher's own read model, not from a metrics backend: the page
 * renders with nothing else running, and the numbers are the same ones the runs
 * list is built from. The window is bounded by retention, which the header
 * states rather than hides.
 */

const searchSchema = z.object({
  ...windowSearchSchema,
  agent_id: z.string().optional(),
  model: z.string().optional(),
});

export const Route = createFileRoute('/observability/metrics')({
  validateSearch: searchSchema,
  component: MetricsPage,
});

/** Token types, in fixed order. Identity, never cycled. */
const TOKEN_SERIES: SeriesDef[] = [
  { key: 'input_tokens', label: 'input', color: SERIES[0] },
  { key: 'output_tokens', label: 'output', color: SERIES[1] },
  { key: 'cached_tokens', label: 'cached', color: SERIES[2] },
];

const RUN_SERIES: SeriesDef[] = [
  { key: 'succeeded', label: 'succeeded', color: SERIES[0] },
  { key: 'failed', label: 'failed', color: 'var(--color-status-critical)' },
];

function MetricsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  // The same presets as every other tab, from `@/components/time-range` —
  // this page had its own list first, and a control that offered different
  // periods here than in Explore made switching tabs a re-read.
  //
  // The window means something slightly different here and deliberately so:
  // it is the timeline's x-axis, so it selects runs by *start*, while the
  // lists select by last activity. A run that began before the axis has no
  // bucket to be counted in.
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;

  const query = useQuery({
    queryKey: ['metrics', windowSeconds, search.agent_id, search.model],
    queryFn: async () => {
      const response = await getMetrics({
        query: {
          window_seconds: windowParam(windowSeconds),
          agent_id: search.agent_id,
          model: search.model,
          buckets: 48,
        },
      });
      if (response.error) throw new Error('failed to load metrics');
      return response.data;
    },
  });

  if (query.isError) {
    return (
      <EmptyState
        title="Could not reach the API"
        hint="Is the aiwatcher server running? The panel proxies /api to it in development."
      />
    );
  }
  if (!query.data) return <p className="text-sm text-muted-foreground">Loading…</p>;

  const metrics = query.data as MetricsSummary;
  const { totals, latency, window } = metrics;
  const billable = totals.input_tokens + totals.output_tokens;
  const successRate = totals.runs > 0 ? totals.succeeded / totals.runs : 0;
  const truncated = window.runs_retained >= window.retention_limit;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Metrics</h1>
          <p className="text-sm text-muted-foreground">
            {window.runs_considered} of {window.runs_retained} retained runs
            {search.agent_id ? ` · agent ${search.agent_id}` : ''}
            {search.model ? ` · model ${search.model}` : ''}
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(seconds) =>
            void navigate({ search: (previous) => ({ ...previous, window: seconds }) })
          }
        />
      </div>

      {truncated ? (
        <Card className="border-warning/40 bg-warning/5">
          <CardContent className="p-3 text-xs text-warning">
            The read model is at its retention limit ({window.retention_limit} runs), so this window
            is a tail rather than the whole history. Longer horizons live in the OTLP metrics.
          </CardContent>
        </Card>
      ) : null}

      {/* Headline numbers. Not charts: a single value reads faster as a value. */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
        <Tile
          label="Runs"
          value={formatCount(totals.runs)}
          hint={`${totals.running} running · ${totals.step_calls} steps`}
        />
        <Tile
          label="Success rate"
          value={`${Math.round(successRate * 100)}%`}
          hint={`${totals.failed} failed`}
          tone={totals.failed === 0 ? 'good' : successRate >= 0.9 ? 'warning' : 'critical'}
        />
        <Tile
          label="Tokens"
          value={formatCount(billable)}
          hint={`${formatCount(totals.input_tokens)} in · ${formatCount(totals.output_tokens)} out`}
        />
        {/*
         * A flat 0% would read as "caching is switched off" when the truth is
         * usually "the provider never reported it". Those are different
         * problems and the tile should not conflate them.
         */}
        <Tile
          label="Cache hit"
          value={totals.cached_tokens > 0 ? `${Math.round(totals.cache_hit_ratio * 100)}%` : '—'}
          hint={
            totals.cached_tokens > 0
              ? `${formatCount(totals.cached_tokens)} cached`
              : 'not reported'
          }
        />
        <Tile
          label="LLM p95"
          value={formatDuration(latency.llm.p95)}
          hint={`${latency.llm.count} calls`}
        />
        <Tile
          label="First token p95"
          value={
            latency.time_to_first_token.count > 0
              ? formatDuration(latency.time_to_first_token.p95)
              : '—'
          }
          hint={
            latency.time_to_first_token.count > 0
              ? `${latency.time_to_first_token.count} streamed`
              : 'no streaming observed'
          }
        />
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Tokens over time</CardTitle>
          </CardHeader>
          <CardContent>
            <StackedBars
              buckets={metrics.timeline.map((bucket) => ({
                at: bucket.at,
                values: {
                  input_tokens: bucket.input_tokens,
                  output_tokens: bucket.output_tokens,
                  cached_tokens: bucket.cached_tokens,
                },
              }))}
              series={TOKEN_SERIES}
              formatValue={formatCount}
              emptyMessage="No token usage in this window."
            />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Runs over time</CardTitle>
          </CardHeader>
          <CardContent>
            <StackedBars
              buckets={metrics.timeline.map((bucket) => ({
                at: bucket.at,
                values: {
                  succeeded: Math.max(bucket.runs - bucket.failed, 0),
                  failed: bucket.failed,
                },
              }))}
              series={RUN_SERIES}
              emptyMessage="No runs in this window."
            />
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>By model</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <RankedBars
              rows={metrics.by_model.map((model): RankedRow => ({
                key: model.model,
                label: model.model,
                value: model.input_tokens + model.output_tokens,
                detail: `${model.calls} calls · p95 ${formatDuration(model.latency.p95)}`,
                warn: model.failures > 0,
              }))}
              formatValue={formatCount}
              emptyMessage="No LLM calls recorded."
            />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>By agent</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <RankedBars
              rows={metrics.by_agent.map((agent): RankedRow => ({
                key: agent.agent_id,
                label: agent.agent_id || '(unnamed)',
                value: agent.input_tokens + agent.output_tokens,
                detail: `${agent.llm_calls} llm · ${agent.tool_calls} tool${agent.failures > 0 ? ` · ${agent.failures} failed` : ''}`,
                warn: agent.failures > 0,
              }))}
              formatValue={formatCount}
              emptyMessage="No agents recorded."
            />
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>By tool</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <RankedBars
              rows={metrics.by_tool.map((tool): RankedRow => ({
                key: tool.tool_name,
                label: tool.tool_name,
                value: tool.calls,
                detail: `p95 ${formatDuration(tool.latency.p95)}${tool.failures > 0 ? ` · ${tool.failures} failed` : ''}`,
                warn: tool.failures > 0,
              }))}
              emptyMessage="No tool calls recorded."
            />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Latency percentiles</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            <PercentileTable
              rows={[
                { label: 'run', percentiles: latency.run },
                { label: 'LLM call', percentiles: latency.llm },
                { label: 'tool call', percentiles: latency.tool },
                { label: 'step', percentiles: latency.step },
                {
                  label: 'first token',
                  percentiles: latency.time_to_first_token,
                },
              ]}
            />
          </CardContent>
        </Card>
      </div>

      {metrics.by_step.length > 0 ? (
        <Card>
          <CardHeader>
            <CardTitle>By step</CardTitle>
          </CardHeader>
          <CardContent className="p-0">
            {/*
             * Ranked by p95, not by call count: a step breakdown is opened to
             * find where the time went. Retrieval latency is invisible in the
             * LLM and tool views and is usually the answer.
             */}
            <RankedBars
              rows={metrics.by_step.map((step): RankedRow => ({
                key: `${step.step_type}:${step.name}`,
                label: `${step.name} · ${step.step_type}`,
                value: Math.round(step.latency.p95),
                detail: `${step.calls} calls${step.failures > 0 ? ` · ${step.failures} failed` : ''}`,
                warn: step.failures > 0,
              }))}
              formatValue={(value) => formatDuration(value)}
              emptyMessage="No steps recorded."
            />
          </CardContent>
        </Card>
      ) : null}
    </div>
  );
}

function Tile({
  label,
  value,
  hint,
  tone,
}: {
  label: string;
  value: string;
  hint?: string;
  tone?: 'good' | 'warning' | 'critical';
}) {
  const color =
    tone === 'good'
      ? 'var(--color-status-good)'
      : tone === 'warning'
        ? 'var(--color-status-warning)'
        : tone === 'critical'
          ? 'var(--color-status-critical)'
          : undefined;
  return (
    <Card>
      <CardContent className="flex flex-col gap-0.5 p-3 pt-3">
        <span className="text-xs uppercase tracking-wide text-muted-foreground">{label}</span>
        <span className="text-xl font-semibold tabular-nums" style={color ? { color } : undefined}>
          {value}
        </span>
        {hint ? <span className="text-xs text-muted-foreground">{hint}</span> : null}
      </CardContent>
    </Card>
  );
}

/**
 * Percentiles as a table, not a chart. Twelve numbers with no ordering to
 * discover is a table's job; a chart would add ink without adding meaning.
 */
function PercentileTable({
  rows,
}: {
  rows: readonly { label: string; percentiles: Percentiles }[];
}) {
  return (
    <table className="w-full text-sm">
      <thead>
        <tr className="border-b border-border text-xs uppercase tracking-wide text-muted-foreground">
          <th className="px-3 py-2 text-left font-medium">Operation</th>
          <th className="px-3 py-2 text-right font-medium">n</th>
          <th className="px-3 py-2 text-right font-medium">p50</th>
          <th className="px-3 py-2 text-right font-medium">p95</th>
          <th className="px-3 py-2 text-right font-medium">p99</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.label} className="border-b border-border/40 last:border-b-0">
            <td className="px-3 py-2">{row.label}</td>
            <td className="px-3 py-2 text-right tabular-nums text-muted-foreground">
              {row.percentiles.count}
            </td>
            {(['p50', 'p95', 'p99'] as const).map((key) => (
              <td key={key} className="px-3 py-2 text-right tabular-nums">
                {row.percentiles.count > 0 ? formatDuration(row.percentiles[key]) : '—'}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
