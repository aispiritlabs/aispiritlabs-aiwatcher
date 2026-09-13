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
  deriveCohort,
  getScorecard,
  getScoringRun,
  listConversationExports,
  listDatasets,
  listExports,
  listScorecards,
  stageBundle,
  stageRecording,
  startScoringRun,
  takeCalibration,
} from '@/api/generated/sdk.gen';
import type {
  CalibrationVersion,
  CohortRequest,
  DatasetKind,
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

/**
 * Where a run's cohort comes from: the chosen result's own, or a dataset
 * version this deployment owns. Either way `limit` takes the owner's first
 * cases — which the server derives, since a subset of a producer's own case
 * file is nothing anybody here could check.
 */
type CohortChoice = {
  from: 'result' | 'dataset';
  kind: Extract<DatasetKind, 'curation' | 'annotations' | 'conversations'>;
  name: string;
  version: string;
  split: string;
  limit: string;
};

const RESULT_COHORT: CohortChoice = {
  from: 'result',
  kind: 'curation',
  name: '',
  version: '',
  split: 'test',
  limit: '',
};

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
  // How the run goes rather than what it measures: neither reaches the
  // manifest, and blank is the deployment's own answer.
  const [pace, setPace] = React.useState({ timeout: '', concurrency: '' });
  const [cohort, setCohort] = React.useState<CohortChoice>(RESULT_COHORT);

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
  const judged: VersionReference[] = (card.data?.scorecard.scorers ?? []).flatMap((spec) =>
    spec.scorer.kind === 'judge' ? [spec.scorer.rubric] : [],
  );
  // The framework metrics the card holds against people, and under what.
  const heldAgainstPeople = (card.data?.scorecard.scorers ?? []).flatMap((spec) =>
    spec.scorer.kind === 'external' && spec.scorer.calibration
      ? [{ metric: spec.metric, calibration: spec.scorer.calibration }]
      : [],
  );
  // One set of people's judgements answers both: it is taken under every
  // rubric the card names, and the server holds each side to its own.
  const rubrics: VersionReference[] = [
    ...judged,
    ...heldAgainstPeople.map((held) => held.calibration.rubric),
  ].filter(
    (rubric, index, all) =>
      all.findIndex((other) => other.name === rubric.name && other.version === rubric.version) ===
      index,
  );
  const asksJudge = judged.length > 0;
  const calibratesFramework = heldAgainstPeople.length > 0;
  // A judge and a scorer service both answer a bounded number at a time, and
  // the declaration's pace is for both.
  const asksElsewhere =
    asksJudge ||
    (card.data?.scorecard.scorers ?? []).some((spec) => spec.scorer.kind === 'external');
  // What each judged metric is shown of the case's question, as the card says.
  const shownInputs = (card.data?.scorecard.scorers ?? []).flatMap((spec) =>
    spec.scorer.kind === 'judge' && spec.input_path !== undefined && spec.input_path !== null
      ? [`${spec.metric} sees the case's input${spec.input_path ? ` at ${spec.input_path}` : ''}`]
      : [],
  );
  const source = published.find((row) => row.receipt.evaluation_id === sourceId);
  const conversations =
    (cohort.from === 'dataset' ? cohort.kind : source?.manifest?.context.dataset.kind) ===
    'conversations';
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
      if ((asksJudge || calibratesFramework) && !calibration) {
        throw new Error(
          asksJudge
            ? 'This card asks a judge: take its calibration set first.'
            : 'This card holds a framework metric against people: take its calibration set first.',
        );
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
      // The result's own cohort as it was, unless a dataset version or a
      // limit was chosen: then the server derives it, and the variant names
      // that dataset, because a manifest's cohort and variant name one.
      let pinned: ScoringRun['cohort'] = {
        case_manifest: manifest.context.case_manifest,
        case_count: manifest.context.case_count,
        split: manifest.context.split,
        input_schema: manifest.context.input_schema,
        expectations_schema: manifest.context.expectations_schema,
      };
      let dataset = manifest.variant.dataset;
      const limit = cohort.limit.trim() ? { limit: Number(cohort.limit) } : {};
      if (cohort.from === 'dataset' || cohort.limit.trim()) {
        if (cohort.from === 'dataset' && (!cohort.name.trim() || !cohort.version.trim())) {
          throw new Error('Choose the dataset version to take the cohort from.');
        }
        const request: CohortRequest =
          cohort.from === 'dataset'
            ? {
                dataset: {
                  kind: cohort.kind,
                  name: cohort.name.trim(),
                  version: cohort.version.trim(),
                },
                split: cohort.split.trim(),
                ...limit,
              }
            : { dataset: manifest.context.dataset, split: manifest.context.split, ...limit };
        pinned = answerOf(
          await deriveCohort({ body: request }),
          'could not take a cohort from that dataset',
        ).cohort;
        dataset = request.dataset;
      }
      const run: ScoringRun = {
        evaluation_id: evaluationId.trim(),
        repetition_id: repetition.trim(),
        variant: { ...manifest.variant, experiment_id: experiment.trim(), dataset },
        cohort: pinned,
        scorecard: { name: head.name, version: head.version },
        answers: staged,
        settings: {
          ...(pace.timeout.trim() ? { timeout_seconds: Number(pace.timeout) * 60 } : {}),
          ...(asksElsewhere && pace.concurrency.trim()
            ? { concurrency: Number(pace.concurrency) }
            : {}),
        },
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
        external_calibration:
          calibratesFramework && calibration
            ? { name: calibration.calibration.name, version: calibration.version }
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
          The same variant pins, with the experiment renamed. The cohort is that result&apos;s
          unless you take one below.
        </span>
      </label>

      <CohortFields value={cohort} onChange={setCohort} />

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
            This card asks a model about {judged.map((rubric) => rubric.name).join(', ')}. The
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

      {calibratesFramework && !asksJudge ? (
        <fieldset className="grid gap-2 rounded border border-border p-3 md:col-span-2">
          <legend>Held against people</legend>
          <p className="text-muted-foreground">
            {heldAgainstPeople
              .map(
                ({ metric, calibration: held }) =>
                  `${metric} passes at ${held.pass_at}, and a person's judgement under ${held.rubric.name} ${
                    held.pass_level ? `at ${held.pass_level}` : 'on its better answer'
                  }`,
              )
              .join('; ')}
            . The result carries how often the two verdicts were the same.
          </p>
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

      <fieldset className="grid gap-2 rounded border border-border p-3 md:col-span-2 md:grid-cols-2">
        <legend>How it runs</legend>
        <label className="flex flex-col gap-1">
          Timeout, minutes
          <input
            aria-label="Timeout in minutes"
            type="number"
            min={1}
            max={1440}
            placeholder={asksJudge ? '60, for a run that asks a judge' : '15, for a fold'}
            className={FIELD}
            value={pace.timeout}
            onChange={(event) => setPace({ ...pace, timeout: event.target.value })}
          />
          <span className="text-muted-foreground">
            Past it the step is stopped and retried like any timeout. Blank is the
            deployment&apos;s.
          </span>
        </label>
        {asksElsewhere ? (
          <label className="flex flex-col gap-1">
            Questions at once
            <input
              aria-label="Judge concurrency"
              type="number"
              min={1}
              max={64}
              placeholder="the deployment's"
              className={FIELD}
              value={pace.concurrency}
              onChange={(event) => setPace({ ...pace, concurrency: event.target.value })}
            />
            <span className="text-muted-foreground">
              Put to a judge or a scorer service at once. More than AIWATCHER_JUDGE_CONCURRENCY or
              AIWATCHER_SCORER_CONCURRENCY allows is refused when the run starts.
            </span>
          </label>
        ) : null}
      </fieldset>

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

/** Which cases a run measures: the result's own, or a dataset version's first ones. */
function CohortFields({
  value,
  onChange,
}: {
  value: CohortChoice;
  onChange: (value: CohortChoice) => void;
}) {
  const fromDataset = value.from === 'dataset';
  const datasets = useQuery({
    queryKey: ['measure-datasets'],
    enabled: fromDataset && value.kind === 'curation',
    queryFn: async () => answerOf(await listDatasets(), 'could not read the datasets'),
    retry: false,
  });
  const project = value.kind === 'annotations' ? value.name.trim() : '';
  const exports = useQuery({
    queryKey: ['measure-annotation-exports', project],
    enabled: fromDataset && Boolean(project),
    queryFn: async () =>
      answerOf(
        await listExports({ query: { name: project } }),
        'could not read that project’s exports',
      ),
    retry: false,
  });
  const corpora = useQuery({
    queryKey: ['measure-conversation-exports'],
    enabled: fromDataset && value.kind === 'conversations',
    queryFn: async () =>
      answerOf(await listConversationExports(), 'could not read the conversation exports'),
    retry: false,
  });
  const chosen = datasets.data?.datasets.find((row) => row.name === value.name);
  const set = (patch: Partial<CohortChoice>) => onChange({ ...value, ...patch });

  return (
    <fieldset className="grid gap-2 rounded border border-border p-3 md:col-span-2 md:grid-cols-3">
      <legend>Cohort</legend>
      <label className="flex items-center gap-2">
        <input
          type="radio"
          name="cohort"
          checked={!fromDataset}
          onChange={() => set({ from: 'result' })}
        />
        The result&apos;s own
      </label>
      <label className="flex items-center gap-2 md:col-span-2">
        <input
          type="radio"
          name="cohort"
          checked={fromDataset}
          onChange={() => set({ from: 'dataset' })}
        />
        A dataset version this deployment owns
      </label>

      {fromDataset ? (
        <>
          <label className="flex flex-col gap-1">
            Kind
            <select
              aria-label="Dataset kind"
              className={FIELD}
              value={value.kind}
              onChange={(event) =>
                set({
                  kind: event.target.value as CohortChoice['kind'],
                  name: '',
                  version: '',
                  split: 'test',
                })
              }
            >
              <option value="curation">curation dataset</option>
              <option value="annotations">annotation export</option>
              <option value="conversations">conversation corpus</option>
            </select>
          </label>
          {value.kind === 'curation' ? (
            <>
              <label className="flex flex-col gap-1">
                Dataset
                <select
                  aria-label="Dataset"
                  className={FIELD}
                  value={value.name}
                  onChange={(event) => set({ name: event.target.value, version: '' })}
                >
                  <option value="">Choose a dataset…</option>
                  {(datasets.data?.datasets ?? []).map((row) => (
                    <option key={row.name} value={row.name}>
                      {row.name}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                Version
                <select
                  aria-label="Dataset version"
                  className={FIELD}
                  value={value.version}
                  onChange={(event) => set({ version: event.target.value })}
                >
                  <option value="">Choose a version…</option>
                  {(chosen?.versions ?? []).map((version) => (
                    <option key={version.version} value={version.version}>
                      {pinchId(version.version, 8, 6)} · {version.row_count} rows
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                Split name
                <input
                  aria-label="Split"
                  className={FIELD}
                  value={value.split}
                  onChange={(event) => set({ split: event.target.value })}
                />
                <span className="text-muted-foreground">
                  A curation version deals no splits: this names the cohort and selects nothing.
                </span>
              </label>
            </>
          ) : null}
          {value.kind === 'annotations' ? (
            <>
              <label className="flex flex-col gap-1">
                Project
                <input
                  aria-label="Annotation project"
                  className={FIELD}
                  value={value.name}
                  onChange={(event) => set({ name: event.target.value, version: '' })}
                />
              </label>
              <label className="flex flex-col gap-1">
                Export
                <select
                  aria-label="Annotation export"
                  className={FIELD}
                  value={value.version}
                  onChange={(event) => set({ version: event.target.value })}
                >
                  <option value="">Choose an export…</option>
                  {(exports.data?.exports ?? []).map((row) => (
                    <option key={row.export} value={row.export}>
                      {pinchId(row.export, 8, 6)} · {row.created_at}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                Split
                <select
                  aria-label="Split"
                  className={FIELD}
                  value={value.split}
                  onChange={(event) => set({ split: event.target.value })}
                >
                  <option value="test">test</option>
                  <option value="validation">validation</option>
                  <option value="train">train</option>
                </select>
              </label>
            </>
          ) : null}
          {value.kind === 'conversations' ? (
            <label className="flex flex-col gap-1 md:col-span-2">
              Corpus
              <select
                aria-label="Conversation corpus"
                className={FIELD}
                value={value.version ? `${value.name}@${value.version}` : ''}
                onChange={(event) => {
                  const [name = '', version = ''] = event.target.value.split('@');
                  set({ name, version, split: 'test' });
                }}
              >
                <option value="">Choose a finished export…</option>
                {(corpora.data?.jobs ?? [])
                  .filter((job) => job.version)
                  .map((job) => (
                    <option key={job.job_id} value={`${job.name}@${job.version}`}>
                      {job.name} @ {pinchId(job.version ?? '', 8, 6)} · {job.conversations}{' '}
                      conversations
                    </option>
                  ))}
              </select>
              <span className="text-muted-foreground">
                A corpus is measured on its test split. Its cases are content: an admin takes a
                cohort from one.
              </span>
            </label>
          ) : null}
          <Refused error={datasets.error ?? exports.error ?? corpora.error} />
        </>
      ) : null}

      <label className="flex flex-col gap-1">
        First cases
        <input
          aria-label="Case limit"
          type="number"
          min={1}
          placeholder="all of them"
          className={FIELD}
          value={value.limit}
          onChange={(event) => set({ limit: event.target.value })}
        />
      </label>
      <span className="self-end text-muted-foreground md:col-span-2">
        The owner&apos;s first cases, in its own order — not a sample. A cohort of some of the cases
        is its own cohort: its results compare with nothing measured on all of them. The server
        derives the three files it pins, so nobody stages them.
      </span>
    </fieldset>
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
  const unheard = 'Acknowledge the warnings first.';
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
                `${metric.name} (${metric.unit}, ${metric.direction}, ${metric.aggregation}${
                  metric.measured_by
                    ? `, by ${metric.measured_by.adapter.name} ${metric.measured_by.adapter.version}${
                        metric.measured_by.model ? ` on ${metric.measured_by.model.name}` : ''
                      }`
                    : ''
                })`,
            )
            .join(', ')}
        </dd>
        <dt className="text-muted-foreground">Cohort</dt>
        <dd>
          {manifest.context.dataset.name} ·{' '}
          {view.data.cohort
            ? `the first ${manifest.context.case_count} of ${view.data.cohort.available} cases`
            : `${manifest.context.case_count} cases`}{' '}
          · split {manifest.context.split}
          {view.data.cohort ? (
            <span className="text-muted-foreground">
              {' '}
              — derived here by {view.data.cohort.derived_by}
            </span>
          ) : null}
        </dd>
        <dt className="text-muted-foreground">Answers</dt>
        <dd>
          {run.answers === 'archive'
            ? "the archive's own responses"
            : `${run.answers.name} (${pinchId(run.answers.digest, 8, 6)})`}
        </dd>
        <dt className="text-muted-foreground">Runs</dt>
        <dd>
          {run.settings?.timeout_seconds
            ? `stopped after ${Math.round(run.settings.timeout_seconds / 60)} min`
            : "the deployment's timeout"}
          {run.judge
            ? ` · ${run.settings?.concurrency ?? "the deployment's number of"} question${
                run.settings?.concurrency === 1 ? '' : 's'
              } at once`
            : ''}
        </dd>
        {run.external_calibration ? (
          <>
            <dt className="text-muted-foreground">People</dt>
            <dd>
              framework metrics held against {run.external_calibration.name} @{' '}
              {pinchId(run.external_calibration.version, 8, 6)}
            </dd>
          </>
        ) : null}
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
              aria-label="Acknowledge the warnings"
              checked={acknowledged}
              onChange={(event) => setAcknowledged(event.target.checked)}
            />
            I have read what this run sends and how its numbers are made. Admitting this pair and
            starting the run commit to both.
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
          : view.cohort
            ? `The manifest is this declaration’s, and the cohort’s three files are derived again from ${view.cohort.request.dataset.name} when the pair is admitted; bring the files the variant pins (${view.manifest.variant.code.name}, ${view.manifest.variant.generation_config.name}).`
            : 'The manifest is this declaration’s; bring the files its cohort and variant pin.'}
      </span>
      <Refused error={admit.error} />
    </form>
  );
}
