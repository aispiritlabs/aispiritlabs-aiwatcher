import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { AlertTriangle, Check, X } from 'lucide-react';

import { listPrompts } from '@/api/generated/sdk.gen';
import type {
  OptimizationSummary,
  PromptVersionSummary,
  RejectionReason,
} from '@/api/generated/types.gen';
import { Badge, IdChip } from '@/shared/components/ui/primitives';
import { formatDuration } from '@/shared/lib/utils';

/**
 * The small pieces the prompts list and the prompt page both render.
 *
 * Together in one file because they are all about one idea — saying what the
 * registry decided and why — and splitting them would put the vocabulary in
 * five places.
 */

/**
 * Why a candidate was refused, in words rather than in an enum name.
 *
 * The reason is doing real work here: "did not improve" invites raising the
 * iteration count, and "dropped a variable" does not. Reading `variables_lost`
 * as a badge would lose exactly the distinction that matters.
 */
export const REJECTION_TEXT: Record<RejectionReason, string> = {
  no_held_out_improvement: 'the held-out score did not improve',
  no_held_out_measurement: 'nothing was measured on the held-out split',
  variables_lost: 'it stopped interpolating a variable the baseline used',
  no_change: 'the candidate is identical to the baseline',
};

/** A version id is `sha256(text)`: 64 lowercase hex, and nothing else. */
const VERSION_ID = /^[0-9a-f]{64}$/;

/**
 * What a run says it ran on, as a link into the registry.
 *
 * The pair is deliberate and neither half is enough. The id identifies the
 * text; the registry is keyed by name, so only the name resolves. An id with no
 * name still tells two runs apart, so it is shown — as a chip rather than a
 * link, because a link that 404s is worse than a fact you have to look up.
 *
 * Never the prompt's text: that is off the log on purpose (ADR_0021).
 */
export function PromptRefLink({ name, versionId }: { name: unknown; versionId: unknown }) {
  if (typeof versionId !== 'string' || !VERSION_ID.test(versionId)) return null;
  const short = versionId.slice(0, 12);
  if (typeof name !== 'string' || name.length === 0) {
    return <IdChip value={short} full={versionId} label="prompt version" />;
  }
  return (
    <Link
      to="/prompts/$name"
      params={{ name }}
      search={{ version: versionId }}
      className="id text-primary hover:underline"
      title={`${name} @ ${versionId}`}
    >
      {name}@{short}
    </Link>
  );
}

/**
 * A prompt a fold grouped by, as a link into the registry.
 *
 * [`PromptRefLink`] is the other half: it starts from a call, which carries a
 * version id, and the id is its admission. This one starts from a *name* — the
 * key of the `prompt` dimension, lifted off `aiwatcher.prompt.name` — and a
 * name is whatever a producer sent. Nothing says the registry holds it: the
 * text is authored and outlives retention (ADR_0011), while the name on a span
 * is retained telemetry, so the two halves can disagree in either direction.
 *
 * So the registry is asked, and a name it does not list is rendered as the
 * fact it is — the same rule as `lineage-reference.tsx`'s, for the same reason.
 * One page is read for every name on a screen, because a registry with more
 * prompts than that page holds leaves a name *unresolved*, which lands on the
 * safe side of the rule rather than on a link that 404s.
 */
export function PromptNameLink({ name }: { name: string }) {
  const registry = useQuery({
    queryKey: ['lineage-prompts'],
    retry: false,
    // No store configured answers 501, which `throwOnError` turns into a
    // failure rather than an empty registry — and an unreadable registry
    // resolves nothing, which is what the unlinked branch below means.
    queryFn: async () => (await listPrompts({ throwOnError: true, query: { limit: 500 } })).data,
    staleTime: 60_000,
  });

  if (!registry.data?.prompts.some((prompt) => prompt.name === name)) {
    return <span title="No registered prompt of this name was confirmed">{name}</span>;
  }
  return (
    <Link to="/prompts/$name" params={{ name }} className="text-primary hover:underline">
      {name}
    </Link>
  );
}

export function OutcomeBadge({ record }: { record: OptimizationSummary }) {
  if (record.outcome === 'admitted') {
    return (
      <Badge tone="success" className="gap-1">
        <Check className="h-3 w-3" />
        admitted
      </Badge>
    );
  }
  const critical = record.reason === 'variables_lost';
  return (
    <Badge tone={critical ? 'danger' : 'neutral'} className="gap-1">
      {critical ? <AlertTriangle className="h-3 w-3" /> : <X className="h-3 w-3" />}
      rejected
    </Badge>
  );
}

/** A signed delta, coloured by direction. `null` renders as an em dash. */
export function Delta({
  value,
  digits = 3,
}: {
  value: number | null | undefined;
  digits?: number;
}) {
  if (value === null || value === undefined)
    return <span className="text-muted-foreground">—</span>;
  const tone =
    value > 1e-12 ? 'text-success' : value < -1e-12 ? 'text-danger' : 'text-muted-foreground';
  return (
    <span className={`tabular-nums ${tone}`}>
      {value > 0 ? '+' : value < 0 ? '−' : '±'}
      {Math.abs(value).toFixed(digits)}
    </span>
  );
}

/**
 * The dev gain against the held-out gain.
 *
 * Side by side and in that order, because the pair is the finding: a candidate
 * that moved 0.35 on the split it was searched against and 0.00 on the split it
 * never saw learned the dev cases, and either number alone hides that.
 */
export function SplitDeltas({ record }: { record: OptimizationSummary }) {
  return (
    <span className="inline-flex items-baseline gap-2 text-xs">
      <span className="text-muted-foreground">dev</span>
      <Delta value={record.dev_delta} />
      <span className="text-muted-foreground">held out</span>
      <Delta value={record.test_delta} />
    </span>
  );
}

/**
 * How far the dev gain outran the held-out one.
 *
 * Flagged past 0.1 rather than merely displayed: a gap that size is the
 * signal that the next optimisation should change the split, not the
 * iteration count.
 */
export function OverfitGap({ gap }: { gap: number | null | undefined }) {
  if (gap === null || gap === undefined) return null;
  const wide = gap > 0.1;
  return (
    <span
      title={
        wide
          ? 'The dev split gained far more than the held-out split — the search found something about the dev cases.'
          : 'Dev and held-out moved together, which is what a real improvement looks like.'
      }
      className={`inline-flex items-center gap-1 text-xs tabular-nums ${
        wide ? 'text-warning' : 'text-muted-foreground'
      }`}
    >
      {wide ? <AlertTriangle className="h-3 w-3" /> : null}
      overfit gap {gap.toFixed(3)}
    </span>
  );
}

/** Who wrote a version: a person, or an optimiser with a name. */
export function OriginBadge({ version }: { version: PromptVersionSummary }) {
  if (version.origin === 'optimized') {
    return (
      <Badge tone="running" title={`produced by ${version.algorithm}`}>
        {version.algorithm}
      </Badge>
    );
  }
  return <Badge tone="neutral">authored</Badge>;
}

/** The `{{ placeholders }}` a version interpolates. */
export function Variables({ names }: { names: string[] }) {
  if (names.length === 0) {
    return (
      <span
        className="text-xs text-muted-foreground"
        title="This prompt interpolates nothing, so nothing about the input reaches the model through it."
      >
        no variables
      </span>
    );
  }
  return (
    <span className="flex flex-wrap gap-1">
      {names.map((name) => (
        <code key={name} className="rounded bg-muted px-1.5 py-0.5 text-[11px]">
          {`{{ ${name} }}`}
        </code>
      ))}
    </span>
  );
}

/** `baseline → candidate`, both copyable. */
export function VersionArrow({ from, to }: { from: string; to: string }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <IdChip value={from.slice(0, 12)} full={from} label="baseline" />
      <span className="text-muted-foreground">→</span>
      <IdChip value={to.slice(0, 12)} full={to} label="candidate" />
    </span>
  );
}

/** Duration and iteration count, where the optimiser reported them. */
export function RunCost({ record }: { record: OptimizationSummary }) {
  const parts = [
    record.iterations === null || record.iterations === undefined
      ? null
      : `${record.iterations} iterations`,
    record.duration_ms === null || record.duration_ms === undefined
      ? null
      : formatDuration(record.duration_ms),
  ].filter(Boolean);
  if (parts.length === 0) return null;
  return <span className="text-xs text-muted-foreground">{parts.join(' · ')}</span>;
}

/**
 * The evaluation report this optimisation published, when it published one.
 *
 * The join between the two halves of the loop: the registry says the prompt
 * changed, the evaluation area says what that did to the scores on every case.
 */
export function EvaluationLink({ evaluationId }: { evaluationId: string | null | undefined }) {
  if (!evaluationId) return null;
  return (
    <Link
      to="/evaluation"
      search={{ report: evaluationId }}
      className="text-xs text-primary hover:underline"
    >
      evaluation report
    </Link>
  );
}
