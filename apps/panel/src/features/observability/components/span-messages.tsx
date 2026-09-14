import { useQuery } from '@tanstack/react-query';
import * as React from 'react';

import { listConversationTurns } from '@/api/generated';
import type { ArchivedTurn } from '@/api/generated/types.gen';
import { isRegistryDisabled } from '@/shared/components/registry-disabled';
import { TurnContentView, useTurnContent } from '@/shared/components/turn-content';
import { Badge, Button } from '@/shared/components/ui/primitives';

/**
 * What was said in this call, where the deployment keeps it.
 *
 * The words are not on the event log and never will be: they live in the
 * encrypted archive with their own retention and their own erasure (ADR_0021),
 * and the log holds only the reference. This joins the two back together by
 * the provenance a producer recorded — the run, and the span of the call — so
 * a trace can show an answer without the archive having to move.
 *
 * Three states, all of them normal:
 *
 * - **No archive.** The routes answer 501 and this says so once, quietly. A
 *   deployment that keeps no content is a configuration, not a failure.
 * - **Nothing recorded.** The archive is on and this call's producer wrote no
 *   turn for it. Said plainly, because the alternative is a reader assuming
 *   the words were lost.
 * - **Recorded.** The heads are listed without decrypting anything; the words
 *   themselves are a second request, which the server answers only for an
 *   admin.
 */
export function SpanMessages({
  conversationId,
  runId,
  spanId,
  reveal,
}: {
  conversationId: string;
  runId: string;
  spanId: string;
  /** From the view menu: open every turn's content as it arrives. */
  reveal: boolean;
}) {
  const turns = useQuery({
    queryKey: ['conversation-turns', conversationId, runId, spanId],
    retry: false,
    queryFn: async () => {
      const response = await listConversationTurns({
        throwOnError: true,
        query: { conversation_id: conversationId, run_id: runId, span_id: spanId, limit: 50 },
      });
      return response.data;
    },
  });

  if (turns.isLoading) return null;
  if (turns.error) {
    return isRegistryDisabled(turns.error) ? (
      <p className="text-xs text-muted-foreground">
        No content archive on this deployment, so nothing holds what was said. Turn it on with
        AIWATCHER_CONVERSATION_ARCHIVE.
      </p>
    ) : (
      <p className="text-xs text-muted-foreground">
        The archive could not be read: {(turns.error as { message?: string }).message ?? 'unknown'}.
      </p>
    );
  }

  const recorded = turns.data?.turns ?? [];
  if (recorded.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        The archive holds nothing for this call. Its producer records telemetry and no words, which
        is the default.
      </p>
    );
  }

  return (
    <ul className="flex flex-col gap-2">
      {recorded.map((turn) => (
        <li key={turn.turn_id}>
          <Message conversationId={conversationId} turn={turn} reveal={reveal} />
        </li>
      ))}
    </ul>
  );
}

function Message({
  conversationId,
  turn,
  reveal,
}: {
  conversationId: string;
  turn: ArchivedTurn;
  reveal: boolean;
}) {
  const [opened, setOpened] = React.useState(false);
  const erased = turn.state === 'erased';
  const wanted = (reveal || opened) && !erased;
  const content = useTurnContent(conversationId, turn.turn_id, wanted);

  return (
    <div className="flex flex-col gap-1.5 rounded border border-border/60 p-2 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <Badge>{turn.role}</Badge>
        <span className="text-muted-foreground">{turn.content_bytes} B</span>
        {(turn.findings ?? []).length > 0 ? (
          <Badge tone="warning">{turn.findings?.length} findings</Badge>
        ) : null}
        {turn.review?.state && turn.review.state !== 'pending' ? (
          <Badge tone={turn.review.state === 'approved' ? 'success' : 'danger'}>
            {turn.review.state}
          </Badge>
        ) : null}
        {erased ? <Badge tone="neutral">erased</Badge> : null}
      </div>

      {erased ? (
        <p className="text-muted-foreground">
          Erased {turn.erasure?.at} ({turn.erasure?.reason}). The digest is what remains of it.
        </p>
      ) : wanted ? (
        <TurnContentView loading={content.isLoading} error={content.error} content={content.data} />
      ) : (
        <Button variant="ghost" size="sm" className="self-start" onClick={() => setOpened(true)}>
          Show what was said
        </Button>
      )}
    </div>
  );
}
