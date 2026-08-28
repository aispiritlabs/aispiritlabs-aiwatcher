import { Badge, Spinner } from '@/components/ui/primitives';
import type { EvaluationStatus, RunStatus } from '@/api/generated/types.gen';

const tone = {
  running: 'running',
  succeeded: 'success',
  failed: 'danger',
} as const;

/**
 * A run and an evaluation reach the same three states, so they share the
 * badge. The two enums are generated separately because the backend keeps the
 * two projections apart; the vocabulary is deliberately identical.
 */
export function StatusBadge({ status }: { status: RunStatus | EvaluationStatus }) {
  return (
    <Badge tone={tone[status] ?? 'neutral'} className="gap-1.5">
      {status === 'running' ? <Spinner /> : null}
      {status}
    </Badge>
  );
}

/**
 * Whether the panel is level with the log.
 *
 * Worth its own indicator: a live view that has silently stopped updating looks
 * exactly like a quiet system, and the difference matters.
 */
export function StreamBadge({ phase }: { phase: 'catching-up' | 'live' | 'reconnecting' }) {
  if (phase === 'live') {
    return (
      <Badge tone="success" className="gap-1.5">
        <span className="h-1.5 w-1.5 rounded-full bg-success" />
        live
      </Badge>
    );
  }
  if (phase === 'catching-up') {
    return (
      <Badge tone="running" className="gap-1.5">
        <Spinner />
        catching up
      </Badge>
    );
  }
  return (
    <Badge tone="warning" className="gap-1.5">
      <Spinner />
      reconnecting
    </Badge>
  );
}
