/**
 * Starting a measurement: one form, and then whatever the server made of it.
 *
 * Everything that decides what a run measures is derived on the server — the
 * metrics from the card, the approval from the pair, the run's identity from
 * the declaration — so this collects the choices a person makes and renders the
 * answer. It computes no address and no metric, for the reason the approvals
 * panel computes no approval ID: a second implementation in TypeScript would be
 * a second answer to what a run measures.
 *
 * The cohort and the variant are taken from a result already published, with
 * the experiment renamed. Neither is typed: both pin artifacts by digest, and a
 * form that asked for digests would be a form nobody fills in correctly.
 *
 * Once declared, the declaration is in the URL, and so is the run it started —
 * a managed run outlives the tab that asked for it.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import * as React from 'react';

import {
  approveSource,
  declareScoringRun,
  getScorecard,
  getScoringRun,
  listScorecards,
  stageBundle,
  stageRecording,
  startScoringRun,
  takeCalibration,
} from '@/api/generated/sdk.gen';
import type {
  CalibrationVersion,
  DurableEvaluation,
  ScoringRun,
  ScoringRunView,
  VersionReference,
} from '@/api/generated/types.gen';
import { ManagedRunCard, useManagedRun } from '@/shared/components/managed-run';
import { needsRole, useRoleDecision } from '@/shared/lib/auth';
import { answerOf, ApiFailure } from '@/shared/lib/result';
import { Badge, Button, Card, IdChip, Spinner } from '@/shared/components/ui/primitives';
import { pinchId } from '@/shared/lib/utils';

import { useEvidence } from './evidence';

const FIELD = 'rounded border border-border bg-background p-1.5';

/** A refusal, with every problem the server listed. */
function Refused({ error }: { error: unknown }) {
  if (!error) return null;
  const failure = error instanceof ApiFailure ? error : undefined;
  return (
    <div className="text-xs text-danger">
      <p>{error instanceof Error ? error.message : String(error)}</p>
      {failure?.details.length ? (
        <ul className="list-disc pl-4">
          {failure.details.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

export function Measure({
  declaration,
  measured,
  onDeclared,
  onStarted,
  onOpenResult,
}: {
  declaration: string | undefined;
  measured: string | undefined;
  onDeclared: (declaration: string | undefined) => void;
  /** The run this declaration started, or none once a missing one is forgotten. */
  onStarted: (execution: string | undefined) => void;
  onOpenResult: (evaluationId: string) => void;
}) {
  return (
    <Card className="flex flex-col gap-3 p-4">
      <div>
        <h2 className="text-sm font-semibold">Measure</h2>
        <p className="text-xs text-muted-foreground">
          Score answers somebody already has against a scorecard, as a managed run that publishes
          its own evidence. No model of the application is called; a judge is, when the card asks
          one.
        </p>
      </div>
      {declaration ? (
        <Declared
          id={declaration}
          measured={measured}
          onStarted={onStarted}
          onOpenResult={onOpenResult}
          onAnother={() => onDeclared(undefined)}
        />
      ) : (
        <Draft onDeclared={onDeclared} />
      )}
    </Card>
  );
}

/** Published results whose manifest can be measured again. */
function usePublished(): DurableEvaluation[] {
  const evidence = useEvidence(undefined);
  return (evidence.data?.pages ?? [])
    .flatMap((page) => page.evaluations)
    .filter((row) => row.manifest && (row.state === 'complete' || row.state === 'partial'));
}

function Draft({ onDeclared }: { onDeclared: (declaration: string) => void }) {
  const editor = useRoleDecision('editor');
  const cards = useQuery({
    queryKey: ['evaluation-scorecards'],
    queryFn: async () => answerOf(await listScorecards(), 'could not read the scorecards'),
    retry: false,
  });
  const published = usePublished();

  const [cardName, setCardName] = React.useState('');
  const [sourceId, setSourceId] = React.useState('');
  const [experiment, setExperiment] = React.useState('');
  const [evaluationId, setEvaluationId] = React.useState('');
  const [repetition, setRepetition] = React.useState('measurement-1');
  const [answers, setAnswers] = React.useState<'recording' | 'archive'>('recording');
  const [recording, setRecording] = React.useState<File | undefined>();
  const [judge, setJudge] = React.useState({
    provider: 'llamacpp',
    model: '',
    revision: '',
    temperature: '0',
    seed: '',
    maxTokens: '256',
    instructions: '',
  });
  const [calibration, setCalibration] = React.useState<CalibrationVersion | undefined>();

  const head = cards.data?.scorecards.find((card) => card.name === cardName);
  const card = useQuery({
    queryKey: ['evaluation-scorecard', cardName, head?.version],
    enabled: Boolean(head),
    queryFn: async () =>
      answerOf(
        await getScorecard({ path: { name: cardName }, query: { version: head?.version } }),
        'could not read this scorecard',
      ),
    retry: false,
  });
  const rubrics: VersionReference[] = (card.data?.scorecard.scorers ?? []).flatMap((spec) =>
    spec.scorer.kind === 'judge' ? [spec.scorer.rubric] : [],
  );
  const asksJudge = rubrics.length > 0;
  // What each judged metric is shown of the case's question, as the card says.
  const shownInputs = (card.data?.scorecard.scorers ?? []).flatMap((spec) =>
    spec.scorer.kind === 'judge' && spec.input_path !== undefined && spec.input_path !== null
      ? [`${spec.metric} sees the case's input${spec.input_path ? ` at ${spec.input_path}` : ''}`]
      : [],
  );
  const source = published.find((row) => row.receipt.evaluation_id === sourceId);
  const conversations = source?.manifest?.context.dataset.kind === 'conversations';
  // The archive is the only place a conversation cohort's answers may come
  // from; the server refuses the other pairing, and the form does not offer it.
  const chosenAnswers = conversations ? 'archive' : answers;

  React.useEffect(() => {
    if (source?.manifest) setExperiment(source.manifest.variant.experiment_id);
  }, [source?.manifest]);

  const take = useMutation({
    mutationFn: async (from: string) =>
      answerOf(
        await takeCalibration({
          body: { name: `people-on-${from}`, evaluation_id: from, rubrics },
        }),
        'could not take a calibration set from that result',
      ),
    onSuccess: setCalibration,
  });

  const declare = useMutation({
    mutationFn: async (): Promise<ScoringRunView> => {
      const manifest = source?.manifest;
      if (!manifest || !head) throw new Error('Choose a scorecard and a result to measure like.');
      if (asksJudge && !calibration) {
        throw new Error('This card asks a judge: take its calibration set first.');
      }
      let staged: ScoringRun['answers'] = 'archive';
      if (chosenAnswers === 'recording') {
        if (!recording) throw new Error('Choose the recording to score.');
        staged = answerOf(
          await stageRecording({
            path: { name: recording.name },
            body: new Uint8Array(await recording.arrayBuffer()) as unknown as Array<number>,
          }),
          'could not stage the recording',
        );
      }
      const run: ScoringRun = {
        evaluation_id: evaluationId.trim(),
        repetition_id: repetition.trim(),
        variant: { ...manifest.variant, experiment_id: experiment.trim() },
        cohort: {
          case_manifest: manifest.context.case_manifest,
          case_count: manifest.context.case_count,
          split: manifest.context.split,
          input_schema: manifest.context.input_schema,
          expectations_schema: manifest.context.expectations_schema,
        },
        scorecard: { name: head.name, version: head.version },
        answers: staged,
        judge:
          asksJudge && calibration
            ? {
                provider: judge.provider,
                model: { name: judge.model.trim(), version: judge.revision.trim() },
                settings: {
                  temperature: Number(judge.temperature),
                  max_tokens: Number(judge.maxTokens),
                  ...(judge.seed.trim() ? { seed: Number(judge.seed) } : {}),
                  ...(judge.instructions.trim() ? { instructions: judge.instructions } : {}),
                },
                calibration: {
                  name: calibration.calibration.name,
                  version: calibration.version,
                },
              }
            : undefined,
      };
      return answerOf(await declareScoringRun({ body: run }), 'the declaration was refused');
    },
    onSuccess: (view) => onDeclared(view.declaration.id),
  });

  return (
    <form
      className="grid gap-3 text-xs md:grid-cols-2"
      onSubmit={(event) => {
        event.preventDefault();
        declare.mutate();
      }}
    >
      <label className="flex flex-col gap-1">
        Scorecard
        <select
          aria-label="Scorecard"
          className={FIELD}
          value={cardName}
          onChange={(event) => setCardName(event.target.value)}
        >
          <option value="">Choose a scorecard…</option>
          {(cards.data?.scorecards ?? []).map((card) => (
            <option key={card.name} value={card.name}>
              {card.name} · {card.metrics.map((metric) => metric.name).join(', ')}
            </option>
          ))}
        </select>
        {head ? (
          <span className="text-muted-foreground">
            Version {pinchId(head.version, 8, 6)} measures{' '}
            {head.metrics
              .map((metric) => `${metric.name} (${metric.unit}, ${metric.direction})`)
              .join(', ')}
            .
          </span>
        ) : null}
      </label>

      <label className="flex flex-col gap-1">
        Measure like
        <select
          aria-label="Measure like"
          className={FIELD}
          value={sourceId}
          onChange={(event) => setSourceId(event.target.value)}
        >
          <option value="">Choose a published result…</option>
          {published.map((row) => (
            <option key={row.receipt.evaluation_id} value={row.receipt.evaluation_id}>
              {row.receipt.evaluation_id} · {row.manifest?.context.dataset.name} ·{' '}
              {row.manifest?.context.case_count} cases
            </option>
          ))}
        </select>
        <span className="text-muted-foreground">
          The same cohort, split and variant pins; only the experiment is renamed.
        </span>
      </label>

      <label className="flex flex-col gap-1">
        Experiment
        <input
          aria-label="Experiment"
          className={FIELD}
          value={experiment}
          onChange={(event) => setExperiment(event.target.value)}
        />
      </label>
      <div className="grid grid-cols-2 gap-2">
        <label className="flex flex-col gap-1">
          Evaluation ID
          <input
            aria-label="Evaluation ID"
            className={FIELD}
            value={evaluationId}
            onChange={(event) => setEvaluationId(event.target.value)}
          />
        </label>
        <label className="flex flex-col gap-1">
          Repetition
          <input
            aria-label="Repetition"
            className={FIELD}
            value={repetition}
            onChange={(event) => setRepetition(event.target.value)}
          />
        </label>
      </div>

      <fieldset className="flex flex-col gap-1 md:col-span-2">
        <legend>Answers</legend>
        <label className="flex items-center gap-2">
          <input
            type="radio"
            name="answers"
            checked={chosenAnswers === 'recording'}
            disabled={conversations}
            onChange={() => setAnswers('recording')}
          />
          A recording
          <input
            type="file"
            accept="application/json"
            aria-label="Recording"
            disabled={chosenAnswers !== 'recording'}
            onChange={(event) => setRecording(event.target.files?.[0])}
            className={FIELD}
          />
        </label>
        <label className="flex items-center gap-2">
          <input
            type="radio"
            name="answers"
            checked={chosenAnswers === 'archive'}
            disabled={!conversations}
            onChange={() => setAnswers('archive')}
          />
          The archive's own responses
          <span className="text-muted-foreground">
            {conversations
              ? '— the only source for a conversation cohort: a recording of these answers would sit outside the archive, unsealed.'
              : '— only for a conversation cohort.'}
          </span>
        </label>
      </fieldset>

      {asksJudge ? (
        <fieldset className="grid gap-2 rounded border border-border p-3 md:col-span-2 md:grid-cols-3">
          <legend>Judge</legend>
          <p className="text-muted-foreground md:col-span-3">
            This card asks a model about {rubrics.map((rubric) => rubric.name).join(', ')}. The
            result will say so, and carry how far the model agreed with the people it is calibrated
            against.{' '}
            {shownInputs.length > 0
              ? `${shownInputs.join('; ')}.`
              : 'It is shown the answer and never what the case asked.'}
          </p>
          {conversations ? (
            <p className="rounded border border-warning/40 bg-warning/5 p-2 text-warning md:col-span-3">
              Over a conversation cohort this judge is sent the archive&apos;s words. Declaring
              sends nothing; the declaration says what will be sent, and starting it asks you to
              acknowledge that.
            </p>
          ) : null}
          <label className="flex flex-col gap-1">
            Profile
            <select
              aria-label="Judge profile"
              className={FIELD}
              value={judge.provider}
              onChange={(event) => setJudge({ ...judge, provider: event.target.value })}
            >
              <option value="llamacpp">llamacpp</option>
              <option value="openai">openai</option>
            </select>
          </label>
          <label className="flex flex-col gap-1">
            Model
            <input
              aria-label="Judge model"
              className={FIELD}
              value={judge.model}
              onChange={(event) => setJudge({ ...judge, model: event.target.value })}
            />
          </label>
          <label className="flex flex-col gap-1">
            Revision
            <input
              aria-label="Judge revision"
              className={FIELD}
              value={judge.revision}
              onChange={(event) => setJudge({ ...judge, revision: event.target.value })}
            />
          </label>
          <label className="flex flex-col gap-1">
            Temperature
            <input
              aria-label="Judge temperature"
              type="number"
              min={0}
              max={2}
              step={0.1}
              className={FIELD}
              value={judge.temperature}
              onChange={(event) => setJudge({ ...judge, temperature: event.target.value })}
            />
          </label>
          <label className="flex flex-col gap-1">
            Seed
            <input
              aria-label="Judge seed"
              type="number"
              min={0}
              className={FIELD}
              value={judge.seed}
              onChange={(event) => setJudge({ ...judge, seed: event.target.value })}
            />
          </label>
          <label className="flex flex-col gap-1">
            Max tokens
            <input
              aria-label="Judge max tokens"
              type="number"
              min={1}
              max={4096}
              className={FIELD}
              value={judge.maxTokens}
              onChange={(event) => setJudge({ ...judge, maxTokens: event.target.value })}
            />
          </label>
          <label className="flex flex-col gap-1 md:col-span-3">
            Instructions, after the rubric's own words
            <textarea
              aria-label="Judge instructions"
              className={FIELD}
              rows={2}
              value={judge.instructions}
              onChange={(event) => setJudge({ ...judge, instructions: event.target.value })}
            />
          </label>
          <Calibration
            published={published}
            exclude={evaluationId.trim()}
            taken={calibration}
            pending={take.isPending}
            error={take.error}
            onTake={(from) => take.mutate(from)}
          />
        </fieldset>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 md:col-span-2">
        <Button
          size="sm"
          type="submit"
          disabled={
            editor === false || declare.isPending || !head || !source || !evaluationId.trim()
          }
          title={editor === false ? needsRole('editor') : undefined}
        >
          {declare.isPending ? 'Declaring…' : 'Declare'}
        </Button>
        <span className="text-muted-foreground">
          Declaring runs nothing. It answers with what the run will publish and the approval that
          admits it.
        </span>
      </div>
      <div className="md:col-span-2">
        <Refused error={declare.error} />
      </div>
    </form>
  );
}

/** Freeze what people judged of one result, for the judge to be held to. */
function Calibration({
  published,
  exclude,
  taken,
  pending,
  error,
  onTake,
}: {
  published: DurableEvaluation[];
  exclude: string;
  taken: CalibrationVersion | undefined;
  pending: boolean;
  error: unknown;
  onTake: (from: string) => void;
}) {
  const [from, setFrom] = React.useState('');
  return (
    <div className="flex flex-col gap-1 md:col-span-3">
      <label className="flex flex-wrap items-center gap-2">
        Calibrate on the people's judgements of
        <select
          aria-label="Calibration result"
          className={FIELD}
          value={from}
          onChange={(event) => setFrom(event.target.value)}
        >
          <option value="">Choose a published result…</option>
          {published
            .filter((row) => row.receipt.evaluation_id !== exclude)
            .map((row) => (
              <option key={row.receipt.evaluation_id} value={row.receipt.evaluation_id}>
                {row.receipt.evaluation_id}
              </option>
            ))}
        </select>
        <Button
          size="sm"
          type="button"
          variant="outline"
          disabled={!from || pending}
          onClick={() => onTake(from)}
        >
          {pending ? 'Taking…' : 'Take calibration set'}
        </Button>
      </label>
      {published.find((row) => row.receipt.evaluation_id === from)?.manifest?.context.dataset
        .kind === 'conversations' ? (
        <span className="text-warning">
          That is conversation evidence: an admin takes its set, and a judge calibrated on it is
          sent the answers people judged there.
        </span>
      ) : null}
      {taken ? (
        <span className="text-muted-foreground">
          {taken.calibration.items.length} human judgement
          {taken.calibration.items.length === 1 ? '' : 's'} of {taken.calibration.result.name}
          {taken.calibration.from_archive ? ', from the conversation archive' : ''}, frozen as{' '}
          <IdChip label="set" value={pinchId(taken.version, 8, 6)} full={taken.version} />
        </span>
      ) : null}
      <Refused error={error} />
    </div>
  );
}

/** A declared run: what it publishes, whether it may, and the run once started. */
function Declared({
  id,
  measured,
  onStarted,
  onOpenResult,
  onAnother,
}: {
  id: string;
  measured: string | undefined;
  onStarted: (execution: string | undefined) => void;
  onOpenResult: (evaluationId: string) => void;
  onAnother: () => void;
}) {
  const editor = useRoleDecision('editor');
  const admin = useRoleDecision('admin');
  const queries = useQueryClient();
  // What the server warned about, heard: nothing is sent until the pair is
  // admitted and the run started, so both of those wait for it.
  const [acknowledged, setAcknowledged] = React.useState(false);
  const followed = useManagedRun(measured);
  const moved = followed.data
    ? `${followed.data.execution.state.state_type}:${followed.data.execution.steps
        .map((step) => step.state.state_type)
        .join(',')}`
    : undefined;
  React.useEffect(() => {
    // The catalogue is re-read whenever the run moves. The step publishes
    // before it settles, so the move that ends the run comes after the result
    // exists — and which states are endings is the server's to say, not a
    // list kept here.
    if (moved) void queries.invalidateQueries({ queryKey: ['evaluation-evidence'] });
  }, [moved, queries]);
  const view = useQuery({
    queryKey: ['evaluation-run', id],
    queryFn: async () =>
      answerOf(await getScoringRun({ path: { id } }), 'could not read this declaration'),
    retry: false,
  });
  const start = useMutation({
    mutationFn: async () =>
      answerOf(await startScoringRun({ path: { id } }), 'the run was not started'),
    onSuccess: (accepted) => onStarted(accepted.execution.execution_id),
  });

  if (view.isLoading) return <Spinner />;
  if (!view.data) return <Refused error={view.error} />;
  const { declaration, manifest, approval_id, admitted } = view.data;
  const { run } = declaration;
  const warnings = view.data.warnings ?? [];
  const heeded = warnings.length === 0 || acknowledged;
  const unheard = 'Acknowledge what this judge is sent first.';
  return (
    <div className="flex flex-col gap-3 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <IdChip label="declaration" value={pinchId(id, 8, 6)} full={id} />
        <IdChip label="approval" value={pinchId(approval_id, 8, 6)} full={approval_id} />
        {admitted ? (
          <Badge tone="primary">admitted</Badge>
        ) : (
          <Badge tone="warning">not admitted</Badge>
        )}
        {manifest.context.judge ? <Badge tone="warning">judged by a model</Badge> : null}
        {manifest.context.judge?.reads_archive ? (
          <Badge tone="danger">judge reads the archive</Badge>
        ) : null}
        <Button size="sm" variant="outline" onClick={onAnother}>
          Declare another
        </Button>
      </div>
      <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
        <dt className="text-muted-foreground">Publishes</dt>
        <dd>
          {run.evaluation_id} · repetition {run.repetition_id} · experiment{' '}
          {run.variant.experiment_id}
        </dd>
        <dt className="text-muted-foreground">Card</dt>
        <dd>
          {run.scorecard.name} @ {pinchId(run.scorecard.version, 8, 6)} —{' '}
          {manifest.context.metrics
            .map(
              (metric) =>
                `${metric.name} (${metric.unit}, ${metric.direction}, ${metric.aggregation})`,
            )
            .join(', ')}
        </dd>
        <dt className="text-muted-foreground">Cohort</dt>
        <dd>
          {manifest.context.dataset.name} · {manifest.context.case_count} cases · split{' '}
          {manifest.context.split}
        </dd>
        <dt className="text-muted-foreground">Answers</dt>
        <dd>
          {run.answers === 'archive'
            ? "the archive's own responses"
            : `${run.answers.name} (${pinchId(run.answers.digest, 8, 6)})`}
        </dd>
        {run.judge ? (
          <>
            <dt className="text-muted-foreground">Judge</dt>
            <dd>
              {run.judge.provider} · {run.judge.model.name} @ {run.judge.model.version} · calibrated
              on {run.judge.calibration.name}
            </dd>
          </>
        ) : null}
      </dl>

      {warnings.length > 0 ? (
        <div
          role="alert"
          className="flex flex-col gap-2 rounded-md border border-danger/40 bg-danger/5 p-3"
        >
          {warnings.map((warning) => (
            <p key={warning} className="text-danger">
              {warning}
            </p>
          ))}
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              aria-label="Acknowledge what this judge is sent"
              checked={acknowledged}
              onChange={(event) => setAcknowledged(event.target.checked)}
            />
            I understand what this judge is sent. Admitting this pair and starting the run send it.
          </label>
        </div>
      ) : null}

      {admitted ? null : (
        <AdmitDeclared
          view={view.data}
          disabled={admin === false || !heeded}
          because={admin === false ? needsRole('admin') : unheard}
          onAdmitted={() => {
            // The refusal a start met before the pair was admitted no longer
            // describes it.
            start.reset();
            void queries.invalidateQueries({ queryKey: ['evaluation-run', id] });
          }}
        />
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          onClick={() => start.mutate()}
          disabled={editor === false || start.isPending || !heeded}
          title={editor === false ? needsRole('editor') : heeded ? undefined : unheard}
        >
          {start.isPending ? 'Starting…' : measured ? 'Start again' : 'Start'}
        </Button>
        {measured ? (
          <>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                // The catalogue was read before this run published into it.
                void queries.invalidateQueries({ queryKey: ['evaluation-evidence'] });
                onOpenResult(run.evaluation_id);
              }}
            >
              Open the result
            </Button>
            <span className="text-muted-foreground">
              Starting again reaches the same run: the declaration is its identity.
            </span>
          </>
        ) : null}
      </div>
      {measured ? (
        <ManagedRunCard
          run={followed.data ?? undefined}
          executionId={measured}
          pending={followed.isLoading}
          missing={followed.data === null}
          failure={followed.error}
          onForget={() => onStarted(undefined)}
        />
      ) : null}
      {start.error instanceof ApiFailure && start.error.code === 'pair_not_admitted' ? (
        <p className="text-danger">
          Nobody has admitted this pair yet. An admin admits approval {pinchId(approval_id, 8, 6)}{' '}
          with the files the cohort and variant pin.
        </p>
      ) : (
        <Refused error={start.error} />
      )}
    </div>
  );
}

/**
 * Stage the pinned files and admit the pair, with the declaration supplied.
 *
 * The manifest is the server's derived one — never written out by hand — so
 * the operator brings only the bytes the cohort and variant pin.
 */
function AdmitDeclared({
  view,
  disabled,
  because,
  onAdmitted,
}: {
  view: ScoringRunView;
  disabled: boolean;
  /** Why it is disabled, when it is. */
  because: string;
  onAdmitted: () => void;
}) {
  const [files, setFiles] = React.useState<File[]>([]);
  const admit = useMutation({
    mutationFn: async () => {
      const declaration = new TextEncoder().encode(JSON.stringify(view.manifest));
      const staged = [
        ...files.map((file) => [file.name, file] as const),
        ['manifest.json', new Blob([declaration])] as const,
      ];
      for (const [name, bytes] of staged) {
        answerOf(
          await stageBundle({
            path: { approval_id: view.approval_id, name },
            body: new Uint8Array(await bytes.arrayBuffer()) as unknown as Array<number>,
          }),
          `could not stage ${name}`,
        );
      }
      return answerOf(
        await approveSource({ body: view.manifest }),
        'the files were staged, and admitting the pair was refused',
      );
    },
    onSuccess: onAdmitted,
  });
  return (
    <form
      className="flex flex-wrap items-center gap-2 rounded border border-border p-3"
      onSubmit={(event) => {
        event.preventDefault();
        admit.mutate();
      }}
    >
      <label className="flex items-center gap-2">
        Pinned files
        <input
          type="file"
          multiple
          aria-label="Pinned files"
          onChange={(event) => setFiles(Array.from(event.target.files ?? []))}
          className={FIELD}
        />
      </label>
      <Button size="sm" type="submit" disabled={disabled || files.length === 0 || admit.isPending}>
        {admit.isPending ? 'Admitting…' : 'Stage and admit'}
      </Button>
      <span className="text-muted-foreground">
        {disabled
          ? because
          : 'The manifest is this declaration’s; bring the files its cohort and variant pin.'}
      </span>
      <Refused error={admit.error} />
    </form>
  );
}
