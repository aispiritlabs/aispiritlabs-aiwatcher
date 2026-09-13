/**
 * Durable evidence, on the screen.
 *
 * The other half of this area: a report folded from the event log is bounded by
 * that log's retention, and evidence published through
 * `/api/v1/evaluation-results` is kept on purpose, with its own clock and its
 * own admission (ADR_0030). They appear in one list, and every row says which
 * it is, because what you can do with them differs.
 *
 * Two rules carry the rest. `EvidenceState::Partial` and `ResultStatus::Partial`
 * arrive together and mean different things — what is *readable* against what
 * was *measured* — so nothing here prints that word twice. And `EvidenceState`
 * is seven answers with seven next steps, of which `forbidden` alone has three
 * causes, so each gets its own sentence rather than one shared "failed" shape.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Fragment, useState } from 'react';

import { getCases, getResult, listResults } from '@/api/generated/sdk.gen';
import type {
  DurableEvaluation,
  EvidenceCase,
  EvidenceState,
  ExternalAgreement,
  GenerationTrace,
  JudgeAgreement,
  JudgeReport,
  ResultCounts,
  ResultStatus,
  RetentionReport,
} from '@/api/generated/types.gen';
import { Badge, Card, EmptyState, IdChip, Spinner, Stat } from '@/shared/components/ui/primitives';
import { useRoleDecision } from '@/shared/lib/auth';
import { ApiFailure, answerOf } from '@/shared/lib/result';
import { cn, formatTime, pinchId } from '@/shared/lib/utils';

import { type CaseFilterChoice, Comparison } from './comparison';
import { CaseJudgement } from './judgement';

const EVIDENCE_PAGE = 50;

/**
 * What each state means and what to do about it.
 *
 * `tone` is the badge's, and only `complete` is quiet: everything else is
 * something a reader has to know before trusting a number, and a grey badge on
 * "the bytes do not verify" reads as a detail.
 */
const EVIDENCE: Record<
  EvidenceState,
  { label: string; what: string; next: string; tone: 'neutral' | 'warning' | 'danger' }
> = {
  complete: {
    label: 'Kept',
    what: 'Every selected case was scored, and the evidence behind it is readable.',
    next: '',
    tone: 'neutral',
  },
  partial: {
    label: 'Kept with gaps',
    what: 'The evidence is here and does not cover every selected case.',
    next: 'The counts below say how many were left unscored or failed.',
    tone: 'warning',
  },
  missing_artifact: {
    label: 'Bytes missing',
    what: 'An object this result points at is no longer in the store. The header is what remains, and nothing can reconstruct the rest.',
    next: 'Measure again under a new evaluation ID — a published result is immutable, so this one cannot be repaired.',
    tone: 'danger',
  },
  corrupt_artifact: {
    label: 'Bytes do not verify',
    what: 'What is stored does not match the digest recorded for it. It is hidden rather than shown, and this is not a deletion.',
    next: 'Check the object store first: a restore that merged two points in time does this. Then measure again under a new ID.',
    tone: 'danger',
  },
  expired: {
    label: 'Retention ran out',
    what: 'The content was deleted when its deadline passed. The receipt stays, so the ID still resolves and never falls back to the event log.',
    next: 'Raise AIWATCHER_EVALUATION_RETENTION_SECONDS before the next measurement, not after this one.',
    tone: 'warning',
  },
  deleted_source: {
    label: 'Source gone',
    what: 'What this was measured against was deleted or revoked at its owner — or this one measurement was forgotten on request. Permanent, either way.',
    next: 'Nothing to do here. Evidence follows the source it was measured against.',
    tone: 'warning',
  },
  forbidden: {
    label: 'Not readable here',
    what: 'Three different things say this, and the server deliberately does not distinguish them on the wire.',
    next: '',
    tone: 'warning',
  },
};

/** The outcome the producer reported, which is not the same question. */
const OUTCOME: Record<ResultStatus, { label: string; tone: 'neutral' | 'warning' | 'danger' }> = {
  succeeded: { label: 'Succeeded', tone: 'neutral' },
  failed: { label: 'Failed', tone: 'danger' },
  // Never the bare word "partial" beside the evidence badge: this one is about
  // what was measured, the other about what is readable.
  partial: { label: 'Partly measured', tone: 'warning' },
};

export function readable(state: EvidenceState): boolean {
  return state === 'complete' || state === 'partial';
}

export function EvidenceBadge({ state }: { state: EvidenceState }) {
  return <Badge tone={EVIDENCE[state].tone}>{EVIDENCE[state].label}</Badge>;
}

function OutcomeBadge({ status }: { status: ResultStatus }) {
  const outcome = OUTCOME[status] ?? { label: status, tone: 'warning' as const };
  return <Badge tone={outcome.tone}>{outcome.label}</Badge>;
}

/**
 * A deadline is a date.
 *
 * `formatTime` is a time of day, which is right for a run that started eleven
 * minutes ago and useless for something that is kept for thirty days — "00:00"
 * is not a deadline anybody can act on.
 */
function keptUntil(expiresAt: number): string {
  return new Date(expiresAt * 1000).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
  });
}

// ── The catalogue ────────────────────────────────────────────────────────────

/**
 * The durable catalogue, paged.
 *
 * No time window. The store's key is the hash of an evaluation ID, so the
 * catalogue's order is the order of a hash — a period control over it would
 * narrow nothing, and one that narrowed the log-folded half of the list and not
 * this half is a control people re-read before every click. See ADR_0030's
 * amendment on when an index over this becomes required.
 */
export function useEvidence(windowSeconds?: number) {
  return useInfiniteQuery({
    queryKey: ['evaluation-evidence', windowSeconds],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      answerOf(
        await listResults({
          query: { cursor: pageParam, limit: EVIDENCE_PAGE, window_seconds: windowSeconds },
        }),
        'could not read the durable evidence catalogue',
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry: false,
  });
}

/**
 * What a store nobody configured looks like.
 *
 * 501 naming the variable, never an empty list: "there is no evidence" and
 * "this instance keeps none" are different facts with different next steps,
 * and the first one is a lie on an instance that was never switched on.
 */
export function EvidenceUnavailable({ failure }: { failure: ApiFailure }) {
  if (failure.status === 501) {
    return (
      <p className="px-3 py-2 text-xs text-muted-foreground">
        This instance keeps no durable evidence. {failure.message} Set{' '}
        <code>AIWATCHER_EVALUATION_SOURCE_DIR</code> and an object store to turn it on — see
        docs/INSTALL.md.
      </p>
    );
  }
  return (
    <p className="px-3 py-2 text-xs text-danger">
      The durable catalogue could not be read: {failure.message} This is not an empty catalogue.
    </p>
  );
}

/** One row, provenance included — the whole reason the two lists are one list. */
export function EvidenceRow({
  evidence,
  selected,
  onSelect,
  gaps,
}: {
  evidence: DurableEvaluation;
  selected: boolean;
  onSelect: () => void;
  gaps?: boolean;
}) {
  const { receipt, state } = evidence;
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'w-full border-b border-border/40 px-3 py-2 text-left transition-colors hover:bg-accent/40',
        selected && 'bg-accent/60',
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-sm font-medium">
          {evidence.manifest?.context.suite.name ?? receipt.evaluation_id}
        </span>
        <EvidenceBadge state={state} />
      </div>
      <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <Badge tone="primary">kept</Badge>
        {/* Not a second state badge: the state is what the header says, and the
            header is readable. This is what the last collection pass found
            behind it, and the two disagreeing is the fact worth showing. */}
        {gaps ? <Badge tone="danger">bytes missing</Badge> : null}
        <span className="tabular-nums">{keptUntil(receipt.committed_at)}</span>
        <span>·</span>
        <span className="truncate">variant {pinchId(receipt.variant_id, 6, 4)}</span>
        {evidence.counts ? (
          <>
            <span>·</span>
            <span className="tabular-nums">
              {evidence.counts.scored}/{evidence.counts.selected} scored
            </span>
          </>
        ) : null}
      </div>
    </button>
  );
}

// ── The detail ───────────────────────────────────────────────────────────────

export function EvidencePane({
  evaluationId,
  gaps,
  baseline,
  onCompare,
  cases,
  onCases,
}: {
  evaluationId: string;
  gaps?: RetentionReport | undefined;
  baseline: string | undefined;
  onCompare: (baseline: string | undefined) => void;
  cases: CaseFilterChoice | undefined;
  onCases: (cases: CaseFilterChoice | undefined) => void;
}) {
  const evidence = useQuery({
    queryKey: ['evaluation-evidence', evaluationId],
    queryFn: async () =>
      answerOf(
        await getResult({ path: { evaluation_id: evaluationId } }),
        'could not read this evidence',
      ),
    retry: false,
  });

  if (evidence.isLoading) {
    return (
      <Card>
        <EmptyState title="Loading…" />
      </Card>
    );
  }
  if (evidence.error instanceof ApiFailure) {
    return (
      <Card className="p-4">
        <EvidenceUnavailable failure={evidence.error} />
      </Card>
    );
  }
  if (!evidence.data) {
    return (
      <Card>
        <EmptyState title="No such evidence" hint="The ID in the URL resolves to nothing kept." />
      </Card>
    );
  }
  return (
    <Evidence
      evidence={evidence.data}
      gaps={gaps}
      baseline={baseline}
      onCompare={onCompare}
      cases={cases}
      onCases={onCases}
    />
  );
}

export function Evidence({
  evidence,
  gaps,
  baseline,
  onCompare,
  cases,
  onCases,
}: {
  evidence: DurableEvaluation;
  gaps?: RetentionReport | undefined;
  baseline?: string | undefined;
  onCompare?: ((baseline: string | undefined) => void) | undefined;
  cases?: CaseFilterChoice | undefined;
  onCases?: ((cases: CaseFilterChoice | undefined) => void) | undefined;
}) {
  const { receipt, state, manifest, counts } = evidence;
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <Card className="p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-base font-semibold">
                {manifest?.context.suite.name ?? receipt.evaluation_id}
              </h2>
              <EvidenceBadge state={state} />
              {evidence.status ? <OutcomeBadge status={evidence.status} /> : null}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <IdChip
                label="evaluation"
                value={pinchId(receipt.evaluation_id, 10, 8)}
                full={receipt.evaluation_id}
              />
              <IdChip
                label="variant"
                value={pinchId(receipt.variant_id, 8, 6)}
                full={receipt.variant_id}
              />
              <IdChip
                label="context"
                value={pinchId(receipt.context_id, 8, 6)}
                full={receipt.context_id}
              />
              <IdChip
                label="version"
                value={pinchId(receipt.version, 8, 6)}
                full={receipt.version}
              />
            </div>
          </div>
        </div>

        <div className="mt-4 grid grid-cols-2 gap-4 md:grid-cols-4">
          <Stat
            label="Published"
            value={keptUntil(receipt.committed_at)}
            hint={formatTime(new Date(receipt.committed_at * 1000).toISOString())}
          />
          {/* A deadline is a dated fact and belongs beside the result: the whole
              point of this store is that it outlives the traces, and how long
              it outlives them by is the first thing somebody needs. */}
          <Stat
            label="Kept until"
            value={keptUntil(receipt.expires_at)}
            hint="content is deleted then; the receipt stays"
          />
          <Stat
            label="Cases"
            value={counts ? `${counts.scored}/${counts.selected}` : '—'}
            hint={counts ? `${counts.failed} failed · ${counts.unscored} unscored` : 'not readable'}
          />
          <Stat label="Split" value={manifest?.context.split ?? '—'} />
        </div>

        <StateNote state={state} counts={counts ?? undefined} />
        {evidence.reproducible || !judgedByARubric(evidence) ? null : (
          <JudgeNote evidence={evidence} />
        )}
        <FrameworkNote evidence={evidence} />
        {evidence.traces ? <TracesNote traces={evidence.traces} /> : null}
        {gaps ? <GapNote report={gaps} /> : null}
      </Card>

      {/* A comparison is offered whatever this result reads as: a delta the
          server withholds because one side is no longer readable is the answer
          somebody came for, and a pane that simply stopped offering it would
          look like a screen that had never had the feature. */}
      {onCompare ? (
        <Comparison
          evidence={evidence}
          baseline={baseline}
          onSelect={onCompare}
          cases={cases}
          onCases={onCases}
        />
      ) : null}

      {readable(state) ? (
        <>
          <Metrics metrics={evidence.metrics} />
          <Cases evaluationId={receipt.evaluation_id} version={receipt.version} />
        </>
      ) : null}
    </div>
  );
}

/**
 * The sentence for this state, and what to do next.
 *
 * `forbidden` is the one with three causes, and the panel is not allowed to
 * guess which: the server answers one word on purpose. What it *can* answer is
 * the caller's own half — reading conversation-derived evidence needs the admin
 * role — so that is stated as a fact when it applies and left out when it does
 * not, the same split the Conversations screens make.
 */
function StateNote({ state, counts }: { state: EvidenceState; counts?: ResultCounts }) {
  const mayReadContent = useRoleDecision('admin');
  if (state === 'complete') return null;
  const note = EVIDENCE[state];
  return (
    <div
      className={cn(
        'mt-3 rounded-md px-3 py-2 text-xs',
        note.tone === 'danger' ? 'bg-danger/10 text-danger' : 'bg-muted/60 text-muted-foreground',
      )}
    >
      <p>{note.what}</p>
      {state === 'partial' && counts ? (
        <p>
          Of {counts.selected} selected cases, {counts.unscored} were never scored and{' '}
          {counts.failed} failed. A partial measurement cannot claim success, and nothing here turns
          a missing score into a zero.
        </p>
      ) : null}
      {state === 'forbidden' ? <ForbiddenCauses mayReadContent={mayReadContent} /> : null}
      {note.next ? <p className="mt-1">{note.next}</p> : null}
    </div>
  );
}

/**
 * Whether a result that re-reading will not reproduce was judged by a rubric
 * judge — as opposed to only by a framework's model, which carries no
 * calibration and says so in its own note.
 */
function judgedByARubric(evidence: DurableEvaluation): boolean {
  if (evidence.judge || evidence.manifest?.context.judge) return true;
  return !(evidence.manifest?.context.metrics ?? []).some((metric) => metric.measured_by?.model);
}

/**
 * What the traces of generated answers showed, as the server counted it.
 *
 * Counts, never a verdict: the run refused answers whose traces contradicted
 * the variant's prompt or model before it published anything, so what is left
 * to say is how much the traces covered — and fewer than all is shown as
 * fewer, not as a failure, because a trace that never arrived contradicts
 * nothing. Whether that is enough is a gate's policy.
 */
function TracesNote({ traces }: { traces: GenerationTrace }) {
  const counted = (value: number | null | undefined) => value ?? traces.answers;
  const complete =
    traces.seen === traces.answers &&
    counted(traces.on_prompt) === traces.answers &&
    counted(traces.on_model) === traces.answers &&
    counted(traces.on_workflow) === traces.answers;
  const pinned = [
    traces.on_prompt == null ? null : `${traces.on_prompt} on the pinned prompt`,
    traces.on_model == null ? null : `${traces.on_model} on the pinned model`,
    traces.on_workflow == null ? null : `${traces.on_workflow} executing the pinned workflow`,
  ].filter(Boolean);
  const served = traces.served ?? [];
  return (
    <div
      className={cn(
        'mt-3 rounded-md px-3 py-2 text-xs',
        complete ? 'bg-muted/60 text-muted-foreground' : 'bg-warning/10 text-warning',
      )}
    >
      <p>
        {`Generated answers, held to their traces: ${traces.seen} of ${traces.answers} seen on the log`}
        {pinned.length > 0 ? `, ${pinned.join(' and ')}` : ''}.
      </p>
      <p>
        {complete
          ? 'Every answer was seen made on what the variant pins that a trace can show.'
          : `${traces.answers - traces.named} named no run; a run the log never received contradicts nothing, and is counted rather than refused.`}
      </p>
      {traces.workflow_undeclared ? (
        <p>
          The declaration of the pinned workflow names no node that could be read, so no run could
          be seen executing it.
        </p>
      ) : null}
      {/* The application's telemetry is its own word; a serving host's run,
          published under another credential, is somebody else's. */}
      {traces.witnessed_model != null ? (
        <p>
          {`${traces.witnessed_model} of ${traces.answers} had a serving host's own run, under another credential, saying it served the call on the pinned model version.`}
        </p>
      ) : null}
      {served.length > 0 ? (
        <p>
          {`What providers said served the calls: ${served
            .map(
              (row) => `${row.model} (${row.answers} ${row.answers === 1 ? 'answer' : 'answers'})`,
            )
            .join(', ')} — compared with nothing.`}
        </p>
      ) : null}
    </div>
  );
}

/**
 * Metrics a scorer framework measured, named where the numbers are.
 *
 * The framework and its release are the server's word, pinned when the card
 * was published. A metric a model graded is a model's word: where the card held
 * it against people, the result carries how often its verdicts were theirs, as
 * a rubric judge's agreement; where it did not, the note says nothing measured
 * that, rather than leaving a gap to be read as agreement.
 */
function FrameworkNote({ evidence }: { evidence: DurableEvaluation }) {
  const context = evidence.manifest?.context;
  const measured = (context?.metrics ?? []).filter((metric) => metric.measured_by);
  if (!context || measured.length === 0) return null;
  const graded = measured.some((metric) => metric.measured_by?.model);
  const calibrated = new Set((evidence.external?.agreement ?? []).map((row) => row.metric));
  return (
    <div
      className={cn(
        'mt-3 rounded-md p-3 text-xs',
        graded ? 'border border-warning/40 bg-warning/5' : 'bg-muted/60 text-muted-foreground',
      )}
    >
      <ul className="flex flex-col gap-1">
        {measured.map((metric) => {
          const by = metric.measured_by;
          if (!by) return null;
          return (
            <li key={metric.name}>
              <span className="font-medium">{metric.name}</span> was measured by {by.adapter.name}{' '}
              {by.adapter.version} ({by.metric})
              {by.model
                ? `, graded by ${by.model.name} @ ${by.model.version} — a model's word, which re-reading will not reproduce${
                    calibrated.has(metric.name)
                      ? '; how often its verdicts were people’s is below.'
                      : ' and whose agreement with people nothing measured.'
                  }`
                : '.'}
            </li>
          );
        })}
      </ul>
      {context.dataset.kind === 'conversations' || context.external_calibration?.reads_archive ? (
        <p className="mt-1 text-danger">
          The scorer service was sent words from the conversation archive
          {graded ? ', and its graded metrics sent them on to their model’s provider' : ''} —
          outside the archive&apos;s encryption, retention and erasure.
        </p>
      ) : null}
      {context.external_calibration ? (
        <>
          <p className="mt-2 text-muted-foreground">
            Held against the people in {context.external_calibration.calibration_dataset.name}: a
            verdict on each side, compared item by item.
          </p>
          {evidence.external ? (
            <>
              <AgreementTable rows={evidence.external.agreement} distance="share" />
              <BeyondTheBar rows={evidence.external.agreement} />
            </>
          ) : (
            <p className="mt-1 text-danger">
              This result carries no agreement with its calibration set.
            </p>
          )}
        </>
      ) : null}
    </div>
  );
}

/**
 * How often a model's word was people's, per metric, over every item.
 *
 * `distance` says what the last column is: a judge's mean distance on the
 * metric's own scale, or — for a framework metric, whose number and a person's
 * judgement meet only as two verdicts — the share of answered items the
 * verdicts differed on.
 */
function AgreementTable({
  rows,
  distance,
}: {
  rows: JudgeAgreement[];
  distance: 'mean' | 'share';
}) {
  return (
    <table className="mt-2 w-full text-left">
      <thead className="text-muted-foreground">
        <tr>
          <th className="font-normal">Metric</th>
          <th className="font-normal">Agreed with people</th>
          <th className="font-normal">95% interval</th>
          <th className="font-normal">Answered</th>
          <th className="font-normal">
            {distance === 'mean' ? 'Mean distance' : 'Verdicts differed'}
          </th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.metric}>
            <td>{row.metric}</td>
            <td>{`${Math.round(row.agreement * 100)}%`}</td>
            <td>
              {row.agreement_interval
                ? `${Math.round(row.agreement_interval.low * 100)}–${Math.round(row.agreement_interval.high * 100)}%`
                : '—'}
            </td>
            <td>{`${row.answered} of ${row.items}`}</td>
            <td>
              {row.mean_absolute_difference === undefined || row.mean_absolute_difference === null
                ? '—'
                : distance === 'mean'
                  ? row.mean_absolute_difference.toFixed(3)
                  : `${Math.round(row.mean_absolute_difference * 100)}%`}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * What a framework metric's calibration says beyond the verdicts at the card's bar.
 *
 * Both are the server's readings, drawn as given and coloured by nothing. The
 * fitted bar was found on the very items it agrees with, so it is labelled as
 * that, with where it lands when the cases are drawn again; adopting it is
 * publishing the card again, which is a new context.
 */
function BeyondTheBar({ rows }: { rows: ExternalAgreement[] }) {
  if (rows.every((row) => row.rank_agreement == null && row.fitted_pass_at == null)) return null;
  const percent = (share: number) => `${Math.round(share * 100)}%`;
  return (
    <table className="mt-2 w-full text-left">
      <thead className="text-muted-foreground">
        <tr>
          <th className="font-normal">Metric</th>
          <th
            className="font-normal"
            title="Goodman and Kruskal's gamma: 1 is the people's order, −1 its reverse"
          >
            Orders answers as people do
          </th>
          <th className="font-normal">Bar these people support</th>
          <th
            className="font-normal"
            title="The same fit made without each case — ten folds of cases in a large set — and scored on what it left out"
          >
            On items it was not fitted on
          </th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.metric}>
            <td>{row.metric}</td>
            <td>
              {row.rank_agreement == null
                ? 'no pair told apart on both sides'
                : `${row.rank_agreement.toFixed(2)} over ${row.ranked_pairs ?? 0} pairs`}
              {row.rank_interval ? (
                <div className="text-muted-foreground">
                  {`${row.rank_interval.low.toFixed(2)} to ${row.rank_interval.high.toFixed(2)}`}
                </div>
              ) : null}
            </td>
            <td>
              {row.fitted_pass_at == null || row.fitted_agreement == null
                ? '—'
                : `${row.fitted_pass_at} would agree ${percent(row.fitted_agreement)} — fitted on these same items`}
              {row.fitted_pass_interval ? (
                <div className="text-muted-foreground">
                  {`${row.fitted_pass_interval.low} to ${row.fitted_pass_interval.high} when the cases are drawn again`}
                </div>
              ) : null}
            </td>
            <td>
              {row.held_out ? (
                <>
                  <div>
                    {`${percent(row.held_out.agreement)} over ${row.held_out.items} items, against ${percent(row.agreement)} at the card's bar`}
                  </div>
                  <div className="text-muted-foreground">
                    {[
                      row.held_out.agreement_interval
                        ? `${percent(row.held_out.agreement_interval.low)}–${percent(row.held_out.agreement_interval.high)}`
                        : null,
                      row.held_out.folds != null
                        ? `${row.held_out.folds} bars fitted${
                            row.held_out.fold_pass_range
                              ? `, from ${row.held_out.fold_pass_range.low} to ${row.held_out.fold_pass_range.high}`
                              : ''
                          }`
                        : `each half's bar: ${(row.held_out.fold_pass_at ?? []).join(' and ')}`,
                    ]
                      .filter(Boolean)
                      .join(' · ')}
                  </div>
                </>
              ) : (
                <span className="text-muted-foreground">every item is one case&apos;s</span>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * Numbers a model gave, and how far to trust it.
 *
 * Every other number on this screen can be had again by re-reading the bytes
 * behind it. These cannot: a judge answered at the time, so the result carries
 * how often it agreed with the people it was calibrated against. Agreement is
 * not a delta and is not coloured — whether 0.8 is good enough is the reader's
 * call, made beside the number.
 */
function JudgeNote({ evidence }: { evidence: DurableEvaluation }) {
  const judge = evidence.manifest?.context.judge;
  const report = evidence.judge;
  return (
    <div className="mt-3 rounded-md border border-warning/40 bg-warning/5 p-3 text-xs">
      <p className="font-medium">
        Measured by a model at the time — re-reading will not reproduce it.
      </p>
      {judge ? (
        <p className="mt-1 text-muted-foreground">
          {judge.provider} · {judge.model.name} @ {judge.model.version}, calibrated on{' '}
          {judge.calibration_dataset.name}.
        </p>
      ) : null}
      {judge?.reads_archive ? (
        <p className="mt-1 text-danger">
          This judge was sent words from the conversation archive, which its provider keeps outside
          the archive&apos;s encryption, retention and erasure.
        </p>
      ) : null}
      {report ? <Served report={report} /> : null}
      {report ? (
        <AgreementTable rows={report.agreement} distance="mean" />
      ) : (
        <p className="mt-1 text-danger">
          This result carries no agreement with its calibration set.
        </p>
      )}
    </div>
  );
}

/**
 * What the provider said answered, beside what the run declared.
 *
 * The declaration is the author's word and nothing can check it; this is the
 * provider's own, read off every reply. Two rows mean the run's answers did not
 * all come from one backend, which is worth saying rather than choosing one.
 */
function Served({ report }: { report: JudgeReport }) {
  if (report.served === undefined || report.served.length === 0) {
    return <p className="mt-1 text-muted-foreground">The provider named nothing it served.</p>;
  }
  return (
    <ul className="mt-1 text-muted-foreground">
      {report.served.map((row) => (
        <li key={`${row.model ?? ''}|${row.fingerprint ?? ''}`}>
          {`The provider said ${row.model ?? 'an unnamed model'}`}
          {row.fingerprint ? ` (${row.fingerprint})` : ''}
          {` answered ${row.replies} ${row.replies === 1 ? 'reply' : 'replies'}.`}
        </li>
      ))}
    </ul>
  );
}

/**
 * What the last collection pass found behind a header that still reads.
 *
 * A summary is one content-addressed object, so a result whose shards are gone
 * reads as kept until somebody opens a page of it. The pass that deletes what a
 * result does not hold has to list what it does, so it already knows — this is
 * that answer, as old as the pass that made it, which is why it is dated.
 */
function GapNote({ report }: { report: RetentionReport }) {
  return (
    <div className="mt-3 rounded-md border border-danger/40 bg-danger/5 p-3 text-xs text-danger">
      <p className="font-medium">Objects this result points at are missing from the store.</p>
      <p className="mt-1">
        Found by the collection pass of{' '}
        {report.collected_at ? keptUntil(report.collected_at) : keptUntil(report.ran_at)}. The
        header above is a separate object and still verifies, which is why the badge beside the
        title does not say so. Open a page below and it will.
      </p>
    </div>
  );
}

function ForbiddenCauses({ mayReadContent }: { mayReadContent: boolean | undefined }) {
  // Still reading the session. A refusal drawn now is one nobody issued.
  if (mayReadContent === undefined) {
    return (
      <p className="mt-1 flex items-center gap-2">
        <Spinner />
        checking what you may read
      </p>
    );
  }
  return (
    <ul className="mt-1 list-disc pl-4">
      {mayReadContent ? (
        <li>Not your role: you hold admin, so this is not the conversation-content gate.</li>
      ) : (
        <li>
          Reading evidence measured on conversation content needs the admin role. If this evidence
          is that, the other two causes do not apply to you.
        </li>
      )}
      <li>
        The approval that admitted this variant and context was withdrawn. Withdrawal hides every
        result measured under that pair and moves no retention deadline.
      </li>
      <li>
        This deployment can no longer establish access to the source: its approval bundle is not
        mounted, or its bytes changed underneath an admitted pair.
      </li>
    </ul>
  );
}

function Metrics({ metrics }: { metrics: Record<string, number> }) {
  const rows = Object.entries(metrics);
  return (
    <Card className="overflow-auto">
      <div className="flex flex-wrap items-center justify-between gap-2 p-4 pb-2">
        <h3 className="text-sm font-semibold">Metrics</h3>
        <span className="text-xs text-muted-foreground">
          aggregated by the server from the declared metric semantics
        </span>
      </div>
      {rows.length === 0 ? (
        <p className="px-4 pb-4 text-xs text-muted-foreground">Nothing was aggregated.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead>
            <tr>
              <th scope="col" className="px-4 py-1.5">
                Metric
              </th>
              <th scope="col" className="px-4 py-1.5 text-right">
                Value
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map(([name, value]) => (
              <tr key={name} className="border-t border-border/40">
                <td className="px-4 py-1.5 text-muted-foreground">{name}</td>
                <td className="px-4 py-1.5 text-right tabular-nums">{value}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

/**
 * The cases, paged by the immutable version.
 *
 * The page carries its own [`EvidenceState`] — a shard is verified when the
 * page it is on is read, so damage shows here rather than in the header, and it
 * arrives as a state rather than as a short page.
 */
function Cases({ evaluationId, version }: { evaluationId: string; version: string }) {
  const cases = useInfiniteQuery({
    queryKey: ['evaluation-evidence-cases', evaluationId, version],
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      answerOf(
        await getCases({
          path: { evaluation_id: evaluationId },
          query: { version, cursor: pageParam, limit: 200 },
        }),
        'could not read this page of cases',
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry: false,
  });

  const pages = cases.data?.pages ?? [];
  const rows: EvidenceCase[] = pages.flatMap((page) => page.cases);
  const damaged = pages.find((page) => !readable(page.state));
  // Which case is open is a disclosure rather than a filter, so it stays here.
  const [open, setOpen] = useState<string | undefined>(undefined);

  return (
    <Card className="overflow-auto">
      <div className="flex flex-wrap items-center justify-between gap-2 p-4 pb-2">
        <h3 className="text-sm font-semibold">Cases</h3>
        <span className="text-xs text-muted-foreground">
          expected answers come from the source, not from the producer
        </span>
      </div>
      {damaged ? (
        <div className="px-4 pb-3">
          <EvidenceBadge state={damaged.state} />
          <StateNote state={damaged.state} />
        </div>
      ) : null}
      {cases.error instanceof ApiFailure ? (
        <EvidenceUnavailable failure={cases.error} />
      ) : rows.length === 0 && !cases.isLoading ? (
        <p className="px-4 pb-4 text-xs text-muted-foreground">No cases on this page.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead>
            <tr>
              <th scope="col" className="px-4 py-1.5">
                Case
              </th>
              <th scope="col" className="px-4 py-1.5">
                Expected
              </th>
              <th scope="col" className="px-4 py-1.5">
                Actual
              </th>
              <th scope="col" className="px-4 py-1.5 text-right">
                Metrics
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const key = `${row.measurement.case_id}/${row.measurement.repetition_id}`;
              return (
                <Fragment key={key}>
                  <tr className="border-t border-border/40 align-top">
                    <td className="px-4 py-1.5 text-muted-foreground">
                      <button
                        type="button"
                        className="underline decoration-dotted underline-offset-2"
                        aria-expanded={open === key}
                        onClick={() => setOpen(open === key ? undefined : key)}
                      >
                        {row.measurement.case_id}
                      </button>
                    </td>
                    <td className="max-w-[16rem] truncate px-4 py-1.5">
                      {JSON.stringify(row.expected)}
                    </td>
                    <td className="max-w-[16rem] truncate px-4 py-1.5">
                      {row.measurement.error ? (
                        <span className="text-danger">{row.measurement.error}</span>
                      ) : (
                        JSON.stringify(row.measurement.actual)
                      )}
                    </td>
                    <td className="px-4 py-1.5 text-right text-xs tabular-nums">
                      {Object.entries(row.measurement.metrics)
                        .map(([name, value]) => `${name} ${value}`)
                        .join(' · ') || '—'}
                    </td>
                  </tr>
                  {/* Judged and proposed from here as from a comparison row: the
                      case route says where every case sits, the first included. */}
                  {open === key ? (
                    <tr className="border-t border-border/20">
                      <td colSpan={4} className="px-4 pb-2 text-xs">
                        <CaseJudgement
                          evaluationId={evaluationId}
                          caseId={row.measurement.case_id}
                          repetitionId={row.measurement.repetition_id}
                          at={row.at ?? undefined}
                        />
                      </td>
                    </tr>
                  ) : null}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      )}
      {cases.hasNextPage ? (
        <div className="p-3">
          <button
            type="button"
            className="text-xs text-primary underline"
            onClick={() => void cases.fetchNextPage()}
            disabled={cases.isFetchingNextPage}
          >
            {cases.isFetchingNextPage ? 'loading…' : 'load the next page'}
          </button>
        </div>
      ) : null}
    </Card>
  );
}

/**
 * Whether retention is running, beside the catalogue it prunes.
 *
 * A sweep that has been failing for a week looks exactly like one that had
 * nothing to do — unless the failures are on the screen. Absent means no pass
 * has ever finished, which is a different fact from "nothing to do".
 */
export function Retention({
  report,
  loading,
}: {
  report: RetentionReport | null | undefined;
  loading?: boolean;
}) {
  // "No pass has ever finished" is a fact about the instance. Saying it while
  // the catalogue is still being read says it about a request in flight.
  if (loading) {
    return (
      <p className="flex items-center gap-2 text-xs text-muted-foreground">
        <Spinner />
        reading the catalogue
      </p>
    );
  }
  if (!report) {
    return (
      <p className="text-xs text-muted-foreground">
        No retention pass has finished on this instance yet.
      </p>
    );
  }
  return (
    <p className={cn('text-xs', report.failures > 0 ? 'text-danger' : 'text-muted-foreground')}>
      Retention last ran {keptUntil(report.ran_at)}{' '}
      {formatTime(new Date(report.ran_at * 1000).toISOString())} — retired {report.retired},
      collected {report.collected}.
      {report.failures > 0
        ? ` ${report.failures} consecutive failures since: ${report.error ?? 'no reason reported'}`
        : ''}
      {(report.damaged_count ?? 0) > 0 ? (
        <span className="text-danger">
          {' '}
          {report.damaged_count} kept {report.damaged_count === 1 ? 'result is' : 'results are'}{' '}
          missing bytes.
        </span>
      ) : null}
    </p>
  );
}
