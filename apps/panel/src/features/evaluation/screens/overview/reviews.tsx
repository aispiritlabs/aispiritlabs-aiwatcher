/**
 * Case review: something somebody noticed, on its way to being a regression case.
 *
 * A proposal names where it was seen — a trace, a span, a case of a result —
 * and the curation dataset it would join. People write what the answer should
 * have been, approve it, and publish the approved cases as a new version of that
 * dataset, which a cohort is then derived from like any other. The rules are the
 * server's: which state may be approved, that an edit takes an approval away,
 * that somebody else's words take the admin role, that a published case does
 * not change. This renders what the server said, including its refusals.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import * as React from 'react';

import { listReviews, proposeCase, publishReviews, reviewCase } from '@/api/generated/sdk.gen';
import type {
  AssessmentTarget,
  CaseReviewAction as ReviewAction,
  CaseReviewContent as ReviewContent,
  CaseReviewItem as ReviewItem,
} from '@/api/generated/types.gen';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  IdChip,
  Spinner,
} from '@/shared/components/ui/primitives';
import { needsRole, useRoleDecision } from '@/shared/lib/auth';
import { answerOf, ApiFailure } from '@/shared/lib/result';
import { pinchId } from '@/shared/lib/utils';

const FIELD = 'rounded border border-border bg-background p-1.5';

/**
 * How a case's judgements open Case review on a dataset's queue.
 *
 * A context rather than a prop, because the judgements sit several panes deep
 * inside the evidence and only the page knows the URL the queue lives in; where
 * nothing provides it, the review is named without a way there.
 */
export const OpenCaseReview = React.createContext<((dataset: string) => void) | undefined>(
  undefined,
);

/** Where a proposal was seen, as the URL carries it from a case or a trace. */
export type ReviewSeed = {
  dataset?: string;
  trace?: string;
  evaluation?: string;
  case?: string;
  repetition?: string;
};

export function Reviews({
  seed,
  onDataset,
}: {
  seed: ReviewSeed;
  onDataset: (dataset: string | undefined) => void;
}) {
  const [draft, setDraft] = React.useState(seed.dataset ?? '');
  const dataset = seed.dataset;
  return (
    <Card className="flex flex-col gap-3 p-4 text-xs">
      <div>
        <h2 className="text-sm font-semibold">Case review</h2>
        <p className="text-muted-foreground">
          Feedback on a trace or a case, on its way to being a regression case: somebody writes what
          the answer should have been, somebody approves it, and the approved cases publish as a new
          version of the dataset they join. Nothing becomes a case on its own.
        </p>
      </div>
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          onDataset(draft.trim() || undefined);
        }}
      >
        <label className="flex items-center gap-2">
          Dataset
          <input
            aria-label="Review dataset"
            className={FIELD}
            value={draft}
            placeholder="capitals"
            onChange={(event) => setDraft(event.target.value)}
          />
        </label>
        <Button size="sm" type="submit" variant="outline">
          Open
        </Button>
      </form>
      {dataset ? (
        <>
          <Propose dataset={dataset} seed={seed} />
          <Queue dataset={dataset} />
        </>
      ) : (
        <p className="text-muted-foreground">Name the curation dataset the cases join.</p>
      )}
    </Card>
  );
}

function Propose({ dataset, seed }: { dataset: string; seed: ReviewSeed }) {
  const editor = useRoleDecision('editor');
  const queries = useQueryClient();
  const [kind, setKind] = React.useState<'trace' | 'case'>(seed.evaluation ? 'case' : 'trace');
  const [trace, setTrace] = React.useState(seed.trace ?? '');
  const [evaluation, setEvaluation] = React.useState(seed.evaluation ?? '');
  const [caseId, setCaseId] = React.useState(seed.case ?? '');
  const [repetition, setRepetition] = React.useState(seed.repetition ?? 'measurement-1');
  const [question, setQuestion] = React.useState('');
  const [answer, setAnswer] = React.useState('');
  const [note, setNote] = React.useState('');
  const [split, setSplit] = React.useState('');
  const [content, setContent] = React.useState<ReviewContent>('written');
  const propose = useMutation({
    mutationFn: async () => {
      const target: AssessmentTarget =
        kind === 'trace'
          ? { kind: 'trace', trace_id: trace.trim() }
          : {
              kind: 'case',
              evaluation_id: evaluation.trim(),
              case_id: caseId.trim(),
              repetition_id: repetition.trim(),
            };
      return answerOf(
        await proposeCase({
          body: {
            dataset,
            target,
            question,
            ...(answer.trim() ? { answer } : {}),
            ...(note.trim() ? { note } : {}),
            ...(split.trim() ? { split: split.trim() } : {}),
            content,
          },
        }),
        'the proposal was refused',
      );
    },
    onSuccess: () => {
      setQuestion('');
      setAnswer('');
      setNote('');
      void queries.invalidateQueries({ queryKey: ['evaluation-reviews', dataset] });
    },
  });
  return (
    <form
      className="grid gap-2 rounded border border-border p-3 md:grid-cols-2"
      onSubmit={(event) => {
        event.preventDefault();
        propose.mutate();
      }}
    >
      <label className="flex flex-col gap-1">
        Seen on
        <select
          aria-label="Seen on"
          className={FIELD}
          value={kind}
          onChange={(event) => setKind(event.target.value as 'trace' | 'case')}
        >
          <option value="trace">a trace</option>
          <option value="case">a case of a result</option>
        </select>
      </label>
      {kind === 'trace' ? (
        <label className="flex flex-col gap-1">
          Trace ID
          <input
            aria-label="Trace ID"
            className={FIELD}
            value={trace}
            onChange={(event) => setTrace(event.target.value)}
          />
        </label>
      ) : (
        <div className="grid grid-cols-3 gap-1">
          <input
            aria-label="Result"
            placeholder="result"
            className={FIELD}
            value={evaluation}
            onChange={(event) => setEvaluation(event.target.value)}
          />
          <input
            aria-label="Case"
            placeholder="case"
            className={FIELD}
            value={caseId}
            onChange={(event) => setCaseId(event.target.value)}
          />
          <input
            aria-label="Repetition"
            placeholder="repetition"
            className={FIELD}
            value={repetition}
            onChange={(event) => setRepetition(event.target.value)}
          />
        </div>
      )}
      <label className="flex flex-col gap-1 md:col-span-2">
        The question
        <textarea
          aria-label="Question"
          className={FIELD}
          value={question}
          onChange={(event) => setQuestion(event.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1">
        What was answered
        <input
          aria-label="Answer given"
          className={FIELD}
          value={answer}
          onChange={(event) => setAnswer(event.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1">
        Why it is a case
        <input
          aria-label="Note"
          className={FIELD}
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1">
        Split it joins
        <input
          aria-label="Split"
          className={FIELD}
          placeholder="test, dev — or none, to join every split"
          value={split}
          onChange={(event) => setSplit(event.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1">
        Whose words
        <select
          aria-label="Whose words"
          className={FIELD}
          value={content}
          onChange={(event) => setContent(event.target.value as ReviewContent)}
        >
          <option value="written">written by a reviewer</option>
          <option value="observed">
            copied from somebody using the application — an admin approves
          </option>
        </select>
      </label>
      <div className="flex flex-wrap items-center gap-2 md:col-span-2">
        <Button
          size="sm"
          type="submit"
          disabled={editor === false || propose.isPending || !question.trim()}
        >
          {propose.isPending ? 'Proposing…' : 'Propose as a case'}
        </Button>
        {editor === false ? (
          <span className="text-muted-foreground">{needsRole('editor')}</span>
        ) : null}
        {propose.error ? (
          <span className="text-danger">{(propose.error as Error).message}</span>
        ) : null}
        {propose.data ? (
          <span className="text-muted-foreground">
            {propose.data.created
              ? 'Proposed.'
              : 'Already under review — the review below is the one it landed on.'}
          </span>
        ) : null}
      </div>
    </form>
  );
}

function Queue({ dataset }: { dataset: string }) {
  const editor = useRoleDecision('editor');
  const queries = useQueryClient();
  const reviews = useQuery({
    queryKey: ['evaluation-reviews', dataset],
    queryFn: async () =>
      answerOf(await listReviews({ query: { dataset } }), 'could not read the reviews'),
    retry: false,
  });
  const publish = useMutation({
    mutationFn: async () =>
      answerOf(await publishReviews({ query: { dataset } }), 'nothing was published'),
    onSuccess: () => {
      void queries.invalidateQueries({ queryKey: ['evaluation-reviews', dataset] });
      void queries.invalidateQueries({ queryKey: ['datasets'] });
    },
  });
  if (reviews.error) {
    const failure = reviews.error instanceof ApiFailure ? reviews.error : undefined;
    return (
      <p className="text-danger">
        {failure?.status === 501
          ? 'This instance keeps no durable evidence, so it holds no reviews.'
          : (reviews.error as Error).message}
      </p>
    );
  }
  if (reviews.isLoading) return <Spinner />;
  const items = reviews.data?.items ?? [];
  const approved = items.filter((item) => item.state === 'approved').length;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          disabled={editor === false || approved === 0 || publish.isPending}
          onClick={() => publish.mutate()}
        >
          {publish.isPending ? 'Publishing…' : `Publish ${approved} approved as a new version`}
        </Button>
        {publish.error ? (
          <span className="text-danger">{(publish.error as Error).message}</span>
        ) : null}
        {publish.data ? (
          <span className="flex items-center gap-1 text-muted-foreground">
            {dataset} now has
            <IdChip
              label="version"
              value={pinchId(publish.data.dataset.dataset.latest.version, 8, 6)}
              full={publish.data.dataset.dataset.latest.version}
            />
            with {publish.data.dataset.dataset.latest.row_count} rows.
          </span>
        ) : null}
      </div>
      {items.length === 0 ? (
        <EmptyState title="Nothing proposed yet" hint="Propose a case above." />
      ) : (
        <ul className="flex flex-col divide-y divide-border/40">
          {items.map((item) => (
            <Item key={item.id} dataset={dataset} item={item} editor={editor} />
          ))}
        </ul>
      )}
    </div>
  );
}

const TONE: Record<ReviewItem['state'], 'neutral' | 'warning' | 'success' | 'danger' | 'primary'> =
  {
    proposed: 'neutral',
    ready: 'warning',
    approved: 'success',
    rejected: 'danger',
    published: 'primary',
  };

function Item({
  dataset,
  item,
  editor,
}: {
  dataset: string;
  item: ReviewItem;
  editor: boolean | undefined;
}) {
  const queries = useQueryClient();
  const [expected, setExpected] = React.useState(item.expected ?? '');
  const [split, setSplit] = React.useState(item.split ?? '');
  const [reason, setReason] = React.useState('');
  const act = useMutation({
    mutationFn: async (action: ReviewAction) =>
      answerOf(
        await reviewCase({ path: { id: item.id }, query: { dataset }, body: action }),
        'the review refused that',
      ),
    onSuccess: () => void queries.invalidateQueries({ queryKey: ['evaluation-reviews', dataset] }),
  });
  const done = item.state === 'published';
  return (
    <li className="flex flex-col gap-1 py-2">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={TONE[item.state]}>{item.state}</Badge>
        <span className="font-medium">{item.question}</span>
        {item.content === 'observed' ? <Badge tone="warning">somebody&apos;s words</Badge> : null}
        {item.content === 'measured' ? <Badge tone="neutral">read from the result</Badge> : null}
      </div>
      <div className="text-muted-foreground">
        {[
          item.answer ? `answered “${item.answer}”` : null,
          item.note || null,
          item.split ? `joins ${item.split}` : 'joins every split',
          item.target.kind === 'trace'
            ? `trace ${pinchId(item.target.trace_id, 8, 4)}`
            : item.target.kind === 'case'
              ? `case ${item.target.case_id} of ${item.target.evaluation_id}`
              : item.target.kind,
          `proposed by ${item.proposed_by}`,
          item.expected_by ? `expected by ${item.expected_by}` : null,
          item.decided_by ? `${item.state} by ${item.decided_by}` : null,
          item.reason ? `because ${item.reason}` : null,
        ]
          .filter(Boolean)
          .join(' · ')}
      </div>
      {done ? (
        <span className="flex items-center gap-1 text-muted-foreground">
          In
          <IdChip
            label="version"
            value={pinchId(item.published_in ?? '', 8, 6)}
            full={item.published_in ?? ''}
          />
          as {`expected “${item.expected ?? ''}”`}
          {item.split ? `, in ${item.split}` : ''}
        </span>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <input
            aria-label={`Expected answer for ${item.question}`}
            className={FIELD}
            placeholder="what the answer should have been"
            value={expected}
            onChange={(event) => setExpected(event.target.value)}
          />
          <input
            aria-label={`Split for ${item.question}`}
            className={FIELD}
            placeholder="split"
            value={split}
            onChange={(event) => setSplit(event.target.value)}
          />
          {/* A split once named is moved, never cleared: absent keeps it. */}
          <Button
            size="sm"
            variant="outline"
            disabled={editor === false || !expected.trim() || act.isPending}
            onClick={() =>
              act.mutate({
                action: 'expect',
                expected,
                ...(split.trim() ? { split: split.trim() } : {}),
              })
            }
          >
            Save expected
          </Button>
          <Button
            size="sm"
            disabled={editor === false || item.state !== 'ready' || act.isPending}
            onClick={() => act.mutate({ action: 'approve' })}
          >
            Approve
          </Button>
          <input
            aria-label={`Reason to reject ${item.question}`}
            className={FIELD}
            placeholder="why not"
            value={reason}
            onChange={(event) => setReason(event.target.value)}
          />
          <Button
            size="sm"
            variant="ghost"
            disabled={editor === false || !reason.trim() || act.isPending}
            onClick={() => act.mutate({ action: 'reject', reason })}
          >
            Reject
          </Button>
          {act.error ? (
            <span className="text-danger">
              {act.error instanceof ApiFailure && act.error.status === 403
                ? 'Approving somebody else’s words needs the admin role.'
                : (act.error as Error).message}
            </span>
          ) : null}
        </div>
      )}
    </li>
  );
}
