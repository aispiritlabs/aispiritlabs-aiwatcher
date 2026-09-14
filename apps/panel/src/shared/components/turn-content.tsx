import { useQuery } from '@tanstack/react-query';

import { conversationTurnContent } from '@/api/generated';
import type { ContentPart, TurnContent } from '@/api/generated/types.gen';
import { Spinner } from '@/shared/components/ui/primitives';

/**
 * What a turn actually said, once somebody asks for it.
 *
 * The words live in the encrypted archive (ADR_0021), never on the event log,
 * and reading them is its own request against its own route — which is what
 * makes "show me the answer" an action a deployment can refuse, log, and
 * later erase. This is the one renderer for them, shared by the review queue
 * and by a trace, so a part kind is drawn the same way wherever it is read.
 */

/** Fetches one turn's content. Nothing is requested until `enabled`. */
export function useTurnContent(conversationId: string, turnId: string, enabled: boolean) {
  return useQuery({
    queryKey: ['conversation-turn-content', turnId],
    enabled,
    retry: false,
    queryFn: async () => {
      const response = await conversationTurnContent({
        throwOnError: true,
        query: { conversation_id: conversationId, turn_id: turnId },
      });
      return response.data;
    },
  });
}

export function TurnContentView({
  loading,
  error,
  content,
}: {
  loading: boolean;
  error: unknown;
  content?: TurnContent;
}) {
  if (loading) return <Spinner />;
  if (error) {
    const body = error as { code?: string; message?: string };
    // The 403 is not a bug and should not read as one: reading content needs
    // the admin role, and saying so is more useful than "request failed".
    return (
      <p className="text-muted-foreground">
        {body.code === 'forbidden'
          ? 'Reading conversation content needs the admin role.'
          : (body.message ?? 'Could not read this turn.')}
      </p>
    );
  }
  if (!content) return null;
  return (
    <div className="flex flex-col gap-2 rounded border border-border bg-muted/40 p-2">
      {(content.parts ?? []).map((part, index) => (
        <Part key={index} part={part} />
      ))}
      {(content.tool_results ?? []).map((result, index) => (
        <div key={index} className="text-muted-foreground">
          <span className="font-medium">
            {result.name} {result.ok ? '✓' : '✗'}
          </span>
          <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap">{result.content}</pre>
        </div>
      ))}
    </div>
  );
}

function Part({ part }: { part: ContentPart }) {
  switch (part.kind) {
    case 'text':
      return <pre className="max-h-72 overflow-auto whitespace-pre-wrap">{part.text}</pre>;
    case 'reasoning':
      return (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap text-muted-foreground">
          {/* Shown to a reviewer and withheld from every export: a provider's
              chain of thought is not the answer. */}
          reasoning · {part.text}
        </pre>
      );
    case 'tool_call':
      return (
        <pre className="overflow-auto whitespace-pre-wrap text-muted-foreground">
          {part.name}({part.arguments})
        </pre>
      );
    case 'redacted':
      return (
        <p className="text-muted-foreground">
          [redacted: {part.reason} · {part.original_bytes} B]
        </p>
      );
    case 'reference':
      return (
        <p className="text-muted-foreground">
          [{part.media_type || 'reference'}] {part.uri}
        </p>
      );
    default:
      return null;
  }
}
