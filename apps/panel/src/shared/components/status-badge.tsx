import { Badge, Spinner } from '@/shared/components/ui/primitives';
import type { EvaluationStatus, RunStatus } from '@/api/generated/types.gen';
import type { StreamPhase } from '@/shared/lib/live';
import { formatAge, isStalled } from '@/shared/lib/utils';

const tone = {
  running: 'running',
  succeeded: 'success',
  failed: 'danger',
} as const;

/**
 * A run and an evaluation reach the same three states, so they share the
 * badge. The two enums are generated separately because the backend keeps the
 * two projections apart; the vocabulary is deliberately identical.
 *
 * `lastEventAt` turns the third state into two readings of it. `running` means
 * no end event arrived, which covers both a run that is working and a run
 * whose producer was killed mid-flight — and only one of those deserves a
 * spinner. Given the run's newest event the badge says which, without the
 * backend having to declare a death it cannot observe. See `STALLED_AFTER_MS`.
 */
export function StatusBadge({
  status,
  lastEventAt,
}: {
  status: RunStatus | EvaluationStatus;
  lastEventAt?: string | null;
}) {
  if (status === 'running' && isStalled(lastEventAt)) {
    return (
      <Badge tone="warning" className="gap-1.5" title={`No events since ${lastEventAt}`}>
        stalled {formatAge(lastEventAt)}
      </Badge>
    );
  }
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
 * exactly like a quiet system, and the difference matters. `revoked` is the
 * sharpest case of that — the stream stopped because access to what it was
 * watching was taken away, and there is nothing to retry — so it is the one
 * phase drawn as a refusal rather than as a state the stream may leave.
 */
export function StreamBadge({ phase }: { phase: StreamPhase }) {
  if (phase === 'revoked') {
    return (
      <Badge tone="danger" className="gap-1.5" title="The grant that opened this stream is gone.">
        <span className="h-1.5 w-1.5 rounded-full bg-destructive" />
        access revoked
      </Badge>
    );
  }
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
