/**
 * What people and judges said about one case, and the way to say it.
 *
 * A score is only readable beside the form it was given on, so this reads the
 * rubric's own scale and offers exactly the answers that scale admits. It
 * renders no verdict of its own: whether a level is good news is the rubric's
 * `direction` to declare, and colouring one here would be the browser deciding
 * what a declaration exists to state.
 *
 * A person's judgement carries no author — the session is the author, and the
 * server refuses one that names somebody else.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import * as React from 'react';

import { getRubric, listAssessments, listRubrics, recordAssessment } from '@/api/generated/sdk.gen';
import type { Assessment, AssessmentValue, RubricVersion, Scale } from '@/api/generated/types.gen';
import { Badge, Button, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import { needsRole, useRoleDecision } from '@/shared/lib/auth';
import { answerOf, ApiFailure } from '@/shared/lib/result';

/** The three answers a scale admits, as one line of text. */
function saidAs(value: AssessmentValue): string {
  return value.type === 'flag' ? (value.value ? 'yes' : 'no') : String(value.value);
}

function on(seconds: number): string {
  return new Date(seconds * 1000).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

export function CaseJudgement({
  evaluationId,
  caseId,
  repetitionId,
}: {
  evaluationId: string;
  caseId: string;
  repetitionId: string;
}) {
  const editor = useRoleDecision('editor');
  const target = {
    kind: 'case',
    evaluation_id: evaluationId,
    case_id: caseId,
    repetition_id: repetitionId,
  } as const;
  const key = ['evaluation-assessments', evaluationId, caseId, repetitionId];
  const judgements = useQuery({
    queryKey: key,
    queryFn: async () =>
      answerOf(await listAssessments({ query: { ...target } }), 'could not read the judgements'),
    retry: false,
  });
  const rubrics = useQuery({
    queryKey: ['evaluation-rubrics'],
    queryFn: async () => answerOf(await listRubrics(), 'could not read the rubrics'),
    retry: false,
  });
  const failure = judgements.error instanceof ApiFailure ? judgements.error : undefined;

  return (
    <div className="mt-2 flex flex-col gap-2 border-t border-border/40 pt-2">
      <p className="font-medium">Judgements</p>
      {failure ? (
        <p className="text-muted-foreground">
          {failure.status === 501
            ? 'This instance keeps no durable evidence, so it holds no judgements.'
            : `Could not read the judgements: ${failure.message}`}
        </p>
      ) : judgements.isLoading ? (
        <Spinner />
      ) : judgements.data?.assessments.length === 0 ? (
        <p className="text-muted-foreground">Nobody has judged this case yet.</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {judgements.data?.assessments.map((assessment) => (
            <Standing key={assessment.standing_id} assessment={assessment} />
          ))}
        </ul>
      )}
      {(rubrics.data?.rubrics.length ?? 0) === 0 ? (
        rubrics.isLoading || failure ? null : (
          <EmptyState
            title="No rubric declared"
            hint="A judgement needs a form to be given on. Publish one to POST /api/v1/evaluation-rubrics."
          />
        )
      ) : (
        <Record
          target={target}
          invalidates={key}
          rubrics={rubrics.data?.rubrics ?? []}
          disabled={editor === false}
        />
      )}
    </div>
  );
}

function Standing({ assessment }: { assessment: Assessment }) {
  return (
    <li className="flex flex-wrap items-baseline gap-2">
      <Badge tone={assessment.source === 'judge' ? 'warning' : 'primary'}>
        {assessment.source}
      </Badge>
      <span className="font-medium">{assessment.rubric}</span>
      <span>{saidAs(assessment.value)}</span>
      <span className="text-muted-foreground">
        {assessment.author} · {on(assessment.recorded_at)}
        {assessment.revision > 1 ? ` · revision ${assessment.revision}` : ''}
      </span>
      {assessment.rationale ? (
        <span className="w-full text-muted-foreground">{assessment.rationale}</span>
      ) : null}
    </li>
  );
}

/**
 * The answers one scale admits, as controls rather than a free field.
 *
 * The scale is read from the rubric version this is about to write, which is
 * also the version the judgement records: a head that moves between rendering
 * the form and answering it would otherwise store an answer under levels
 * nobody was shown.
 */
function Answers({
  scale,
  value,
  onChange,
}: {
  scale: Scale;
  value: AssessmentValue | undefined;
  onChange: (value: AssessmentValue) => void;
}) {
  if (scale.kind === 'ordinal') {
    return (
      <span className="flex flex-wrap gap-1">
        {scale.levels.map((level) => (
          <Button
            key={level}
            size="sm"
            variant={
              value?.type === 'level' && value.value === level ? 'default' : 'outline'
            }
            onClick={() => onChange({ type: 'level', value: level })}
          >
            {level}
          </Button>
        ))}
      </span>
    );
  }
  if (scale.kind === 'flag') {
    return (
      <span className="flex gap-1">
        {[true, false].map((flag) => (
          <Button
            key={String(flag)}
            size="sm"
            variant={value?.type === 'flag' && value.value === flag ? 'default' : 'outline'}
            onClick={() => onChange({ type: 'flag', value: flag })}
          >
            {flag ? 'yes' : 'no'}
          </Button>
        ))}
      </span>
    );
  }
  return (
    <input
      type="number"
      aria-label="Score"
      min={scale.min}
      max={scale.max}
      step="any"
      value={value?.type === 'number' ? value.value : ''}
      onChange={(event) =>
        onChange({ type: 'number', value: Number(event.target.value) })
      }
      className="w-24 rounded-md border border-border bg-background px-2 py-1"
    />
  );
}

function Record({
  target,
  invalidates,
  rubrics,
  disabled,
}: {
  target: { kind: 'case'; evaluation_id: string; case_id: string; repetition_id: string };
  invalidates: unknown[];
  rubrics: { name: string; version: string; question: string }[];
  disabled: boolean;
}) {
  const queries = useQueryClient();
  const [name, setName] = React.useState(rubrics[0]?.name ?? '');
  const [value, setValue] = React.useState<AssessmentValue | undefined>(undefined);
  const [rationale, setRationale] = React.useState('');
  const chosen = rubrics.find((rubric) => rubric.name === name) ?? rubrics[0];
  const form = useQuery({
    queryKey: ['evaluation-rubric', chosen?.name, chosen?.version],
    queryFn: async () =>
      answerOf(
        await getRubric({
          path: { name: chosen?.name ?? '' },
          query: { version: chosen?.version },
        }),
        'could not read this rubric',
      ),
    enabled: chosen !== undefined,
    retry: false,
  });
  const record = useMutation({
    mutationFn: async (form: RubricVersion) =>
      answerOf(
        await recordAssessment({
          body: {
            target,
            rubric: form.rubric.name,
            rubric_version: form.version,
            value: value as AssessmentValue,
            rationale: rationale.trim(),
          },
        }),
        'could not record this judgement',
      ),
    onSuccess: () => {
      setValue(undefined);
      setRationale('');
      void queries.invalidateQueries({ queryKey: invalidates });
    },
  });

  return (
    <div className="flex flex-col gap-2 rounded-md bg-muted/40 px-3 py-2">
      <span className="flex flex-wrap items-center gap-2">
        <select
          aria-label="Rubric"
          value={chosen?.name ?? ''}
          onChange={(event) => {
            setName(event.target.value);
            setValue(undefined);
          }}
          className="rounded-md border border-border bg-background px-1.5 py-1"
        >
          {rubrics.map((rubric) => (
            <option key={rubric.name} value={rubric.name}>
              {rubric.name}
            </option>
          ))}
        </select>
        {form.data ? (
          <Answers scale={form.data.rubric.scale} value={value} onChange={setValue} />
        ) : (
          <Spinner />
        )}
      </span>
      {form.data ? (
        <p className="text-muted-foreground">{form.data.rubric.question}</p>
      ) : null}
      <textarea
        aria-label="Why"
        rows={2}
        value={rationale}
        onChange={(event) => setRationale(event.target.value)}
        placeholder="why (optional)"
        className="rounded-md border border-border bg-background px-2 py-1"
      />
      <span className="flex items-center gap-2">
        <Button
          size="sm"
          disabled={disabled || value === undefined || !form.data || record.isPending}
          title={disabled ? needsRole('editor') : undefined}
          onClick={() => form.data && record.mutate(form.data)}
        >
          {record.isPending ? 'recording…' : 'Record'}
        </Button>
        {record.error ? (
          <span className="text-danger">{(record.error as Error).message}</span>
        ) : null}
      </span>
    </div>
  );
}
