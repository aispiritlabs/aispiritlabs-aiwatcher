import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Eraser, Eye, KeyRound, ShieldAlert } from 'lucide-react';
import { z } from 'zod';

import {
  conversationPolicy,
  conversationTurnContent,
  eraseConversationContent,
  listConversationArchive,
  listConversationTurns,
  reviewConversationTurn,
} from '@/api/generated';
import type {
  ArchivedTurn,
  ContentPart,
  FindingKind,
  PreferenceLabel,
  TurnContent,
  TurnReviewState,
} from '@/api/generated/types.gen';
import { ArchiveDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Spinner,
} from '@/components/ui/primitives';
import { cn } from '@/lib/utils';

/**
 * The gate: nothing reaches a corpus until somebody has read it.
 *
 * Three things about this screen are deliberate and easy to get wrong.
 *
 * **The list decrypts nothing.** Everything on the left and everything in a
 * turn's header — role, ordering, policy, findings, expiry — comes from the
 * plaintext head. Content is fetched one turn at a time, by an explicit click,
 * and only for a caller with the `admin` role. A queue that pre-loaded every
 * message would make the encryption ornamental.
 *
 * **A rejection needs a reason.** The server refuses one without a note or a
 * finding, and this renders that refusal rather than reimplementing it — the
 * same rule the annotation canvas keeps about the registry's 422.
 *
 * **The preference is not the review state.** Approving says the content may be
 * used; the preference says which of two answers was better. They are separate
 * because a turn rejected for holding somebody's address must never become the
 * rejected half of a preference pair.
 */

const searchSchema = z.object({
  conversation: z.string().optional(),
  review: z.enum(['pending', 'approved', 'rejected']).optional(),
  finding: z.enum(['pii', 'secret', 'unsafe', 'duplicate', 'policy']).optional(),
});

export const Route = createFileRoute('/conversations/review')({
  validateSearch: searchSchema,
  component: ReviewPage,
});

const FINDING_TONE: Record<FindingKind, 'danger' | 'warning' | 'neutral'> = {
  secret: 'danger',
  pii: 'danger',
  unsafe: 'danger',
  duplicate: 'warning',
  policy: 'warning',
};

function ReviewPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();

  const policy = useQuery({
    queryKey: ['conversation-policy'],
    retry: false,
    queryFn: async () => {
      const response = await conversationPolicy({ throwOnError: true });
      return response.data;
    },
  });

  const archive = useQuery({
    queryKey: ['conversation-archive'],
    retry: false,
    enabled: !policy.isError,
    queryFn: async () => {
      const response = await listConversationArchive({ throwOnError: true });
      return response.data.conversations;
    },
  });

  if (isRegistryDisabled(policy.error) || isRegistryDisabled(archive.error)) {
    return <ArchiveDisabled />;
  }

  const conversations = archive.data ?? [];
  const selected = search.conversation ?? conversations[0]?.conversation_id;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Review</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Conversation content, held encrypted and on its own retention clock. Nothing here
            reaches a corpus until it is approved.
          </p>
        </div>
        {policy.data ? (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Badge tone={policy.data.mode === 'protected' ? 'success' : 'warning'}>
              {policy.data.mode}
            </Badge>
            <span>at most {policy.data.max_ttl_days} days</span>
            <span className="flex items-center gap-1">
              <KeyRound className="h-3 w-3" />
              {policy.data.key_ids.join(', ') || 'no key'}
            </span>
          </div>
        ) : null}
      </div>

      <div className="grid gap-4 lg:grid-cols-[20rem_1fr]">
        <ConversationList
          conversations={conversations}
          loading={archive.isLoading}
          selected={selected}
          onSelect={(conversation) =>
            navigate({ search: (previous) => ({ ...previous, conversation }) })
          }
          onErased={() => {
            void queryClient.invalidateQueries({ queryKey: ['conversation-archive'] });
            void queryClient.invalidateQueries({ queryKey: ['conversation-turns'] });
          }}
        />
        {selected ? (
          <TurnList conversationId={selected} review={search.review} finding={search.finding} />
        ) : (
          <EmptyState
            title="Nothing has been archived yet"
            hint="A producer records turns with POST /api/v1/conversation-turns, carrying the consent and retention that permit it."
          />
        )}
      </div>
    </div>
  );
}

type ConversationRow = {
  conversation_id: string;
  turns: number;
  pending: number;
  approved: number;
  rejected: number;
  erased: number;
  findings?: { [key: string]: number };
  last_seen: string;
  earliest_expiry?: string | null;
};

function ConversationList({
  conversations,
  loading,
  selected,
  onSelect,
  onErased,
}: {
  conversations: ConversationRow[];
  loading: boolean;
  selected?: string;
  onSelect: (conversationId: string) => void;
  onErased: () => void;
}) {
  const erase = useMutation({
    mutationFn: async (conversationId: string) => {
      const response = await eraseConversationContent({
        throwOnError: true,
        body: { conversation_id: conversationId },
      });
      return response.data;
    },
    onSuccess: onErased,
  });

  return (
    <Card className="h-fit">
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle className="text-sm">Conversations</CardTitle>
        {loading ? <Spinner /> : null}
      </CardHeader>
      <CardContent className="flex flex-col gap-1 p-2">
        {conversations.length === 0 && !loading ? (
          <p className="p-2 text-xs text-muted-foreground">Nothing archived.</p>
        ) : null}
        {conversations.map((conversation) => (
          <button
            key={conversation.conversation_id}
            type="button"
            onClick={() => onSelect(conversation.conversation_id)}
            className={cn(
              'flex flex-col gap-1 rounded px-2 py-2 text-left text-xs transition-colors hover:bg-muted',
              conversation.conversation_id === selected && 'bg-muted',
            )}
          >
            <span className="truncate font-medium">{conversation.conversation_id}</span>
            <span className="flex flex-wrap items-center gap-1 text-muted-foreground">
              {conversation.pending > 0 ? (
                <Badge tone="warning">{conversation.pending} pending</Badge>
              ) : null}
              {conversation.approved > 0 ? (
                <Badge tone="success">{conversation.approved} approved</Badge>
              ) : null}
              {conversation.rejected > 0 ? <Badge>{conversation.rejected} rejected</Badge> : null}
              {conversation.erased > 0 ? (
                <Badge tone="neutral">{conversation.erased} erased</Badge>
              ) : null}
              {Object.entries(conversation.findings ?? {}).map(([kind, count]) => (
                <Badge key={kind} tone={FINDING_TONE[kind as FindingKind] ?? 'neutral'}>
                  {count} {kind}
                </Badge>
              ))}
            </span>
          </button>
        ))}
        {selected ? (
          <div className="mt-2 border-t border-border pt-2">
            <Button
              variant="ghost"
              size="sm"
              className="w-full justify-start text-danger"
              disabled={erase.isPending}
              onClick={() => {
                if (
                  window.confirm(
                    `Erase every message in ${selected}? Any published corpus that read it loses its rows too. The heads, digests, review decisions and export manifests stay; the words do not come back.`,
                  )
                ) {
                  erase.mutate(selected);
                }
              }}
            >
              <Eraser className="mr-1 h-3 w-3" />
              Erase this conversation
            </Button>
            {erase.isError ? (
              <p className="px-2 pt-1 text-xs text-danger">{(erase.error as Error).message}</p>
            ) : null}
            {erase.data ? (
              <p className="px-2 pt-1 text-xs text-muted-foreground">
                {erase.data.turns_erased} erased, {erase.data.already_erased} already gone
                {erase.data.corpora_withdrawn
                  ? `, ${erase.data.corpora_withdrawn} published ${
                      erase.data.corpora_withdrawn === 1 ? 'corpus' : 'corpora'
                    } withdrawn`
                  : ''}
                .
              </p>
            ) : null}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

function TurnList({
  conversationId,
  review,
  finding,
}: {
  conversationId: string;
  review?: TurnReviewState;
  finding?: FindingKind;
}) {
  const navigate = Route.useNavigate();
  const turns = useQuery({
    queryKey: ['conversation-turns', conversationId, review, finding],
    retry: false,
    queryFn: async () => {
      const response = await listConversationTurns({
        throwOnError: true,
        query: { conversation_id: conversationId, review, finding, limit: 100 },
      });
      return response.data;
    },
  });

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <FilterChips
          label="Review"
          options={['pending', 'approved', 'rejected'] as const}
          value={review}
          onChange={(next) => navigate({ search: (previous) => ({ ...previous, review: next }) })}
        />
        <FilterChips
          label="Finding"
          options={['secret', 'pii', 'unsafe', 'duplicate', 'policy'] as const}
          value={finding}
          onChange={(next) => navigate({ search: (previous) => ({ ...previous, finding: next }) })}
        />
        {turns.data ? (
          <span className="ml-auto text-muted-foreground">
            {turns.data.total} {turns.data.total === 1 ? 'turn' : 'turns'}
          </span>
        ) : null}
      </div>

      {turns.isLoading ? <Spinner /> : null}
      {turns.data?.turns.length === 0 ? <EmptyState title="No turn matches those filters" /> : null}
      {turns.data?.turns.map((turn) => (
        <TurnCard key={turn.turn_id} turn={turn} />
      ))}
    </div>
  );
}

function FilterChips<T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly T[];
  value?: T;
  onChange: (next: T | undefined) => void;
}) {
  return (
    <span className="flex items-center gap-1">
      <span className="text-muted-foreground">{label}</span>
      {options.map((option) => (
        <button
          key={option}
          type="button"
          onClick={() => onChange(value === option ? undefined : option)}
          className={cn(
            'rounded border border-border px-2 py-0.5 transition-colors hover:bg-muted',
            value === option && 'border-primary text-primary',
          )}
        >
          {option}
        </button>
      ))}
    </span>
  );
}

function TurnCard({ turn }: { turn: ArchivedTurn }) {
  const queryClient = useQueryClient();
  const [revealed, setRevealed] = React.useState(false);
  const [note, setNote] = React.useState('');
  const [preference, setPreference] = React.useState<PreferenceLabel | undefined>(
    turn.review?.preference ?? undefined,
  );

  const content = useQuery({
    queryKey: ['conversation-turn-content', turn.turn_id],
    enabled: revealed,
    retry: false,
    queryFn: async () => {
      const response = await conversationTurnContent({
        throwOnError: true,
        query: { conversation_id: turn.conversation_id, turn_id: turn.turn_id },
      });
      return response.data;
    },
  });

  const decide = useMutation({
    mutationFn: async (state: TurnReviewState) => {
      const response = await reviewConversationTurn({
        throwOnError: true,
        body: {
          conversation_id: turn.conversation_id,
          turn_id: turn.turn_id,
          review: { state, note, preference },
        },
      });
      return response.data;
    },
    onSuccess: () => {
      setNote('');
      void queryClient.invalidateQueries({ queryKey: ['conversation-turns'] });
      void queryClient.invalidateQueries({ queryKey: ['conversation-archive'] });
    },
  });

  const erased = turn.state === 'erased';

  return (
    <Card>
      <CardHeader className="flex flex-row flex-wrap items-center gap-2">
        <Badge>{turn.role}</Badge>
        <CardTitle className="text-sm font-normal text-muted-foreground">
          {turn.message_id}
          {turn.parent_message_id ? ` ← ${turn.parent_message_id}` : ''}
        </CardTitle>
        <span className="ml-auto flex flex-wrap items-center gap-2">
          <Badge
            tone={
              turn.review?.state === 'approved'
                ? 'success'
                : turn.review?.state === 'rejected'
                  ? 'danger'
                  : 'warning'
            }
          >
            {turn.review?.state ?? 'pending'}
          </Badge>
          {turn.review?.preference ? <Badge tone="neutral">{turn.review.preference}</Badge> : null}
          {erased ? <Badge tone="neutral">erased</Badge> : null}
        </span>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-xs">
        {/* A compact strip rather than `Stat`: these are five short strings a
            reviewer scans, not headline numbers, and rendering them at heading
            size would make the policy shout over the content it is about. */}
        <dl className="flex flex-wrap gap-x-6 gap-y-1">
          {(
            [
              ['size', `${turn.content_bytes} B`],
              ['basis', turn.policy.consent?.basis ?? 'unknown'],
              ['scope', (turn.policy.consent?.scope ?? []).join(', ') || 'none'],
              ['retention', `${turn.policy.retention?.ttl_days ?? 0} days`],
              ['redactor', turn.policy.redaction?.redactor ?? 'none declared'],
              ['subject', turn.policy.consent?.subject || '—'],
            ] as const
          ).map(([label, value]) => (
            <div key={label} className="flex items-baseline gap-1.5">
              <dt className="text-muted-foreground">{label}</dt>
              <dd className="font-medium">{value}</dd>
            </div>
          ))}
        </dl>

        {turn.retention_clamped ? (
          <p className="text-warning">
            This deployment shortened the retention the producer asked for.
          </p>
        ) : null}

        {(turn.findings ?? []).length > 0 ? (
          <div className="flex flex-wrap items-center gap-1">
            <ShieldAlert className="h-3 w-3 text-danger" />
            {(turn.findings ?? []).map((item, index) => (
              <Badge key={`${item.rule}-${index}`} tone={FINDING_TONE[item.kind] ?? 'neutral'}>
                {item.rule} · {item.found_by}
              </Badge>
            ))}
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-2 text-muted-foreground">
          {(turn.parts ?? []).map((part, index) => (
            <span key={index} className="rounded bg-muted px-1.5 py-0.5">
              {part.kind} · {part.bytes} B
            </span>
          ))}
          {turn.tool_results ? <span>{turn.tool_results} tool results</span> : null}
        </div>

        {erased ? (
          <p className="text-muted-foreground">
            Erased {turn.erasure?.at} ({turn.erasure?.reason}
            {turn.erasure?.by ? ` · ${turn.erasure.by}` : ''}). The digest above is what remains of
            it.
          </p>
        ) : revealed ? (
          <RevealedContent
            loading={content.isLoading}
            error={content.error}
            content={content.data}
          />
        ) : (
          <Button
            variant="ghost"
            size="sm"
            className="self-start"
            onClick={() => setRevealed(true)}
          >
            <Eye className="mr-1 h-3 w-3" />
            Reveal content
          </Button>
        )}

        {!erased ? (
          <div className="flex flex-col gap-2 border-t border-border pt-3">
            <div className="flex flex-wrap items-center gap-2">
              <input
                value={note}
                onChange={(event) => setNote(event.target.value)}
                placeholder="Why — required to reject"
                className="min-w-0 flex-1 rounded border border-border bg-background px-2 py-1"
              />
              <FilterChips
                label="Better answer?"
                options={['chosen', 'rejected'] as const}
                value={preference}
                onChange={setPreference}
              />
            </div>
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                disabled={decide.isPending}
                onClick={() => decide.mutate('approved')}
              >
                Approve
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={decide.isPending}
                onClick={() => decide.mutate('rejected')}
              >
                Reject
              </Button>
              {decide.isError ? (
                <span className="text-danger">{(decide.error as Error).message}</span>
              ) : null}
            </div>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

function RevealedContent({
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
