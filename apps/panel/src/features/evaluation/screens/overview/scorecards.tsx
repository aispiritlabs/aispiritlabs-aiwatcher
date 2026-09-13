/**
 * The cards a measurement is declared against, and the form that publishes one.
 *
 * A scorecard names scorers from a vocabulary the server implements — the ones
 * compiled in, a rubric judge, and the metrics a scorer service's catalog
 * offers — and the server derives everything else: which way each metric is
 * better, its unit, and for a framework's metric the release and the model it
 * is pinned to. So this form builds a well-typed body and nothing more. It
 * sends no `declared`, works out no direction and checks no rule; the server's
 * refusal is rendered as it came.
 *
 * The catalog is read from where the work role recorded it. A deployment with
 * no scorer service answers 404, which is a fact about the deployment rather
 * than a failure: the framework kind is offered and says why it cannot be used.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import * as React from 'react';

import {
  getRubric,
  getScorerCatalog,
  listRubrics,
  listScorecards,
  publishScorecard,
} from '@/api/generated/sdk.gen';
import type {
  RecordedCatalog,
  RubricHead,
  Scorecard,
  ScorecardVersion,
  Scorer,
  ScorerCatalogMetric,
  ScorerSpec,
} from '@/api/generated/types.gen';
import { needsRole, useRoleDecision } from '@/shared/lib/auth';
import { answerOf, answerOrNone, ApiFailure } from '@/shared/lib/result';
import { Button, Card, EmptyState, IdChip, Spinner } from '@/shared/components/ui/primitives';
import { pinchId } from '@/shared/lib/utils';

const FIELD = 'rounded border border-border bg-background p-1.5';

const KINDS: { kind: Scorer['kind']; label: string }[] = [
  { kind: 'exact_match', label: 'Exact match' },
  { kind: 'contains', label: 'Contains the expected text' },
  { kind: 'regex_match', label: 'Matches a pattern' },
  { kind: 'numeric_within', label: 'Number within a tolerance' },
  { kind: 'absolute_error', label: 'Absolute error' },
  { kind: 'forbidden', label: 'Forbidden phrase' },
  { kind: 'judge', label: 'A judge, on a rubric' },
  { kind: 'external', label: "A framework's metric" },
];

/** One scorer as the form holds it: every kind's fields, and the kind that decides. */
type Row = {
  metric: string;
  kind: Scorer['kind'];
  answerPath: string;
  expectedPath: string;
  showsInput: boolean;
  inputPath: string;
  ignoreCase: boolean;
  trim: boolean;
  pattern: string;
  tolerance: string;
  unit: string;
  text: string;
  /** `name@version`, for a judge's rubric. */
  rubric: string;
  passLevel: string;
  adapter: string;
  frameworkMetric: string;
  /** Every parameter as typed; booleans as `''`, `'true'` or `'false'`. */
  parameters: Record<string, string>;
  calibrate: boolean;
  calibrationRubric: string;
  passAt: string;
  calibrationLevel: string;
  /** A numeric rubric's passing score, as typed. */
  calibrationScore: string;
};

function blank(): Row {
  return {
    metric: '',
    kind: 'exact_match',
    answerPath: '',
    expectedPath: '',
    showsInput: false,
    inputPath: '',
    ignoreCase: false,
    trim: false,
    pattern: '',
    tolerance: '0',
    unit: '',
    text: '',
    rubric: '',
    passLevel: '',
    adapter: '',
    frameworkMetric: '',
    parameters: {},
    calibrate: false,
    calibrationRubric: '',
    passAt: '',
    calibrationLevel: '',
    calibrationScore: '',
  };
}

function reference(pinned: string) {
  const at = pinned.lastIndexOf('@');
  return { name: pinned.slice(0, at), version: pinned.slice(at + 1) };
}

function number(value: string, what: string): number {
  const parsed = Number(value);
  if (value.trim() === '' || !Number.isFinite(parsed)) throw new Error(`${what} is a number.`);
  return parsed;
}

/** The metric a row names in the catalog, when it names one. */
function described(catalog: RecordedCatalog | null | undefined, row: Row) {
  const adapter = catalog?.catalog.adapters.find((entry) => entry.name === row.adapter);
  return {
    adapter,
    metric: adapter?.metrics.find((entry) => entry.metric === row.frameworkMetric),
  };
}

/**
 * The body the server reads. Shapes only — a number that is not one, a choice
 * not made — because every rule about what a card may say is the server's.
 */
function specOf(row: Row, catalog: RecordedCatalog | null | undefined): ScorerSpec {
  const metric = row.metric.trim();
  if (!metric) throw new Error('Every scorer names the metric it writes.');
  let scorer: Scorer;
  switch (row.kind) {
    case 'exact_match':
      scorer = { kind: 'exact_match', ignore_case: row.ignoreCase, trim: row.trim };
      break;
    case 'contains':
      scorer = { kind: 'contains', ignore_case: row.ignoreCase };
      break;
    case 'regex_match':
      scorer = { kind: 'regex_match', pattern: row.pattern };
      break;
    case 'numeric_within':
      scorer = {
        kind: 'numeric_within',
        tolerance: number(row.tolerance, `${metric}'s tolerance`),
      };
      break;
    case 'absolute_error':
      scorer = { kind: 'absolute_error', unit: row.unit.trim() };
      break;
    case 'forbidden':
      scorer = { kind: 'forbidden', text: row.text, ignore_case: row.ignoreCase };
      break;
    case 'judge':
      if (!row.rubric) throw new Error(`Choose the rubric ${metric} is judged on.`);
      scorer = {
        kind: 'judge',
        rubric: reference(row.rubric),
        ...(row.passLevel ? { pass_level: row.passLevel } : {}),
      };
      break;
    case 'external': {
      const { metric: offered } = described(catalog, row);
      if (!offered) throw new Error(`Choose the framework metric ${metric} is measured by.`);
      const parameters: Record<string, unknown> = {};
      for (const [name, parameter] of Object.entries(offered.parameters ?? {})) {
        const typed = (row.parameters[name] ?? '').trim();
        if (typed === '') continue;
        parameters[name] =
          parameter.kind === 'boolean'
            ? typed === 'true'
            : parameter.kind === 'number' || parameter.kind === 'integer'
              ? number(typed, `${metric}'s ${name}`)
              : parameter.kind === 'string_list'
                ? typed
                    .split(',')
                    .map((item) => item.trim())
                    .filter(Boolean)
                : typed;
      }
      if (row.calibrate && !row.calibrationRubric) {
        throw new Error(`Choose the rubric people judged ${metric} under.`);
      }
      scorer = {
        kind: 'external',
        adapter: row.adapter,
        metric: row.frameworkMetric,
        ...(Object.keys(parameters).length > 0 ? { parameters } : {}),
        ...(row.calibrate
          ? {
              calibration: {
                rubric: reference(row.calibrationRubric),
                pass_at: number(row.passAt, `${metric}'s passing number`),
                ...(row.calibrationLevel ? { pass_level: row.calibrationLevel } : {}),
                ...(row.calibrationScore.trim()
                  ? {
                      pass_score: number(
                        row.calibrationScore,
                        `the score a person's judgement of ${metric} passes at`,
                      ),
                    }
                  : {}),
              },
            }
          : {}),
      };
      break;
    }
  }
  const reads = row.kind === 'external' ? described(catalog, row).metric?.reads : undefined;
  const showsInput =
    row.kind === 'judge' ? row.showsInput : row.kind === 'external' && reads?.includes('input');
  return {
    metric,
    scorer,
    ...(row.answerPath ? { answer_path: row.answerPath } : {}),
    ...(row.expectedPath && readsExpected(row, reads) ? { expected_path: row.expectedPath } : {}),
    ...(showsInput ? { input_path: row.inputPath } : {}),
  };
}

function readsExpected(row: Row, reads: ScorerCatalogMetric['reads'] | undefined): boolean {
  if (row.kind === 'regex_match' || row.kind === 'forbidden') return false;
  if (row.kind === 'external') return reads?.includes('expected') ?? false;
  return true;
}

export function Scorecards() {
  const cards = useQuery({
    queryKey: ['evaluation-scorecards'],
    queryFn: async () => answerOf(await listScorecards(), 'could not read the scorecards'),
    retry: false,
  });
  const failure = cards.error instanceof ApiFailure ? cards.error : undefined;
  const heads = cards.data?.scorecards ?? [];
  return (
    <Card className="flex flex-col gap-3 p-4">
      <div>
        <h2 className="text-sm font-semibold">Scorecards</h2>
        <p className="text-xs text-muted-foreground">
          What a measurement is declared against. A card names its scorers; which way each metric is
          better, and for a framework&apos;s metric the release and model it is pinned to, are the
          server&apos;s to say. Publishing the same card twice lands on the same version.
        </p>
      </div>
      {failure ? (
        <p className="text-xs text-danger">
          {failure.status === 501
            ? 'This instance keeps no durable evidence, so it holds no scorecards.'
            : `Could not read the scorecards: ${failure.message}`}
        </p>
      ) : cards.isLoading ? (
        <Spinner />
      ) : heads.length === 0 ? (
        <EmptyState title="No scorecard yet" hint="Publish one below." />
      ) : (
        <ul className="flex flex-col divide-y divide-border/40 text-xs">
          {heads.map((head) => (
            <li key={head.name} className="flex flex-col gap-1 py-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{head.name}</span>
                <IdChip label="version" value={pinchId(head.version, 8, 6)} full={head.version} />
              </div>
              <span className="text-muted-foreground">
                {head.metrics
                  .map(
                    (metric) =>
                      `${metric.name} (${metric.unit}, ${metric.direction}${
                        metric.measured_by
                          ? `, by ${metric.measured_by.adapter.name} ${metric.measured_by.adapter.version}`
                          : ''
                      })`,
                  )
                  .join(' · ')}
              </span>
            </li>
          ))}
        </ul>
      )}
      <Author />
    </Card>
  );
}

function Author() {
  const editor = useRoleDecision('editor');
  const queries = useQueryClient();
  const [name, setName] = React.useState('');
  const [description, setDescription] = React.useState('');
  const [rows, setRows] = React.useState<Row[]>([blank()]);
  const catalog = useQuery({
    queryKey: ['evaluation-scorer-catalog'],
    queryFn: async () =>
      answerOrNone(await getScorerCatalog(), 'could not read the scorer service catalog'),
    retry: false,
  });
  const rubrics = useQuery({
    queryKey: ['evaluation-rubrics'],
    queryFn: async () => answerOf(await listRubrics(), 'could not read the rubrics'),
    retry: false,
  });
  const publish = useMutation({
    mutationFn: async (): Promise<ScorecardVersion> => {
      const card: Scorecard = {
        name: name.trim(),
        ...(description.trim() ? { description: description.trim() } : {}),
        scorers: rows.map((row) => specOf(row, catalog.data)),
      };
      return answerOf(await publishScorecard({ body: card }), 'the scorecard was refused');
    },
    onSuccess: () => void queries.invalidateQueries({ queryKey: ['evaluation-scorecards'] }),
  });
  const change = (index: number, patch: Partial<Row>) =>
    setRows((current) => current.map((row, at) => (at === index ? { ...row, ...patch } : row)));

  return (
    <form
      className="flex flex-col gap-3 rounded border border-border p-3 text-xs"
      onSubmit={(event) => {
        event.preventDefault();
        publish.mutate();
      }}
    >
      <div className="grid gap-2 md:grid-cols-2">
        <label className="flex flex-col gap-1">
          Card name
          <input
            aria-label="Card name"
            className={FIELD}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label className="flex flex-col gap-1">
          Description
          <input
            aria-label="Card description"
            className={FIELD}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
        </label>
      </div>
      {rows.map((row, index) => (
        <ScorerFields
          key={index}
          index={index}
          row={row}
          catalog={catalog.data}
          catalogFailed={catalog.error}
          rubrics={rubrics.data?.rubrics ?? []}
          onChange={(patch) => change(index, patch)}
          onRemove={
            rows.length > 1
              ? () => setRows((current) => current.filter((_, at) => at !== index))
              : undefined
          }
        />
      ))}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          type="button"
          variant="outline"
          onClick={() => setRows((current) => [...current, blank()])}
        >
          Add a scorer
        </Button>
        <Button
          size="sm"
          type="submit"
          disabled={editor === false || publish.isPending || !name.trim()}
          title={editor === false ? needsRole('editor') : undefined}
        >
          {publish.isPending ? 'Publishing…' : 'Publish card'}
        </Button>
      </div>
      {publish.error ? (
        <p className="text-danger">
          {publish.error instanceof Error ? publish.error.message : String(publish.error)}
        </p>
      ) : null}
      {publish.data ? <Published version={publish.data} /> : null}
    </form>
  );
}

/** What the server made of it: the version, and what it pinned for each framework metric. */
function Published({ version }: { version: ScorecardVersion }) {
  const pinned = version.scorecard.scorers.flatMap((spec) =>
    spec.scorer.kind === 'external' && spec.scorer.declared
      ? [
          `${spec.metric} is ${spec.scorer.adapter} ${spec.scorer.declared.version}'s ${spec.scorer.metric} (${spec.scorer.declared.unit}, ${spec.scorer.declared.direction}${
            spec.scorer.declared.model
              ? `, graded by ${spec.scorer.declared.model.name} @ ${spec.scorer.declared.model.version}`
              : ''
          })`,
        ]
      : [],
  );
  return (
    <div className="flex flex-col gap-1 text-muted-foreground">
      <span>
        Published {version.scorecard.name} at{' '}
        <IdChip label="version" value={pinchId(version.version, 8, 6)} full={version.version} />
      </span>
      {pinned.length > 0 ? <span>Pinned from the catalog: {pinned.join('; ')}.</span> : null}
    </div>
  );
}

function ScorerFields({
  index,
  row,
  catalog,
  catalogFailed,
  rubrics,
  onChange,
  onRemove,
}: {
  index: number;
  row: Row;
  catalog: RecordedCatalog | null | undefined;
  catalogFailed: unknown;
  rubrics: RubricHead[];
  onChange: (patch: Partial<Row>) => void;
  onRemove: (() => void) | undefined;
}) {
  const label = (what: string) => `Scorer ${index + 1} ${what}`;
  const { adapter, metric: offered } = described(catalog, row);
  const reads = offered?.reads;
  return (
    <fieldset className="grid gap-2 rounded border border-border/60 p-2 md:grid-cols-3">
      <legend>Scorer {index + 1}</legend>
      <label className="flex flex-col gap-1">
        Metric
        <input
          aria-label={label('metric')}
          className={FIELD}
          value={row.metric}
          onChange={(event) => onChange({ metric: event.target.value })}
        />
      </label>
      <label className="flex flex-col gap-1">
        Kind
        <select
          aria-label={label('kind')}
          className={FIELD}
          value={row.kind}
          onChange={(event) => onChange({ kind: event.target.value as Scorer['kind'] })}
        >
          {KINDS.map((option) => (
            <option key={option.kind} value={option.kind}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      <label className="flex flex-col gap-1">
        Answer path
        <input
          aria-label={label('answer path')}
          className={FIELD}
          placeholder="whole answer"
          value={row.answerPath}
          onChange={(event) => onChange({ answerPath: event.target.value })}
        />
      </label>
      {readsExpected(row, reads) ? (
        <label className="flex flex-col gap-1">
          Expected path
          <input
            aria-label={label('expected path')}
            className={FIELD}
            placeholder={row.kind === 'judge' ? 'shown nothing' : 'whole expectation'}
            value={row.expectedPath}
            onChange={(event) => onChange({ expectedPath: event.target.value })}
          />
        </label>
      ) : null}

      {row.kind === 'exact_match' || row.kind === 'contains' || row.kind === 'forbidden' ? (
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            aria-label={label('ignores case')}
            checked={row.ignoreCase}
            onChange={(event) => onChange({ ignoreCase: event.target.checked })}
          />
          Ignore case
        </label>
      ) : null}
      {row.kind === 'exact_match' ? (
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            aria-label={label('trims')}
            checked={row.trim}
            onChange={(event) => onChange({ trim: event.target.checked })}
          />
          Trim whitespace
        </label>
      ) : null}
      {row.kind === 'regex_match' ? (
        <TextField
          label={label('pattern')}
          title="Pattern"
          value={row.pattern}
          onChange={(pattern) => onChange({ pattern })}
        />
      ) : null}
      {row.kind === 'numeric_within' ? (
        <TextField
          label={label('tolerance')}
          title="Tolerance"
          value={row.tolerance}
          onChange={(tolerance) => onChange({ tolerance })}
        />
      ) : null}
      {row.kind === 'absolute_error' ? (
        <TextField
          label={label('unit')}
          title="Unit it counts in"
          value={row.unit}
          onChange={(unit) => onChange({ unit })}
        />
      ) : null}
      {row.kind === 'forbidden' ? (
        <TextField
          label={label('phrase')}
          title="Phrase"
          value={row.text}
          onChange={(text) => onChange({ text })}
        />
      ) : null}

      {row.kind === 'judge' ? (
        <>
          <RubricChoice
            label={label('rubric')}
            title="Rubric"
            rubrics={rubrics}
            value={row.rubric}
            onChange={(rubric) => onChange({ rubric, passLevel: '' })}
          />
          <LevelChoice
            label={label('level to reach')}
            title="Level to reach"
            rubric={row.rubric}
            value={row.passLevel}
            onChange={(passLevel) => onChange({ passLevel })}
            none="the mean position"
          />
          <InputChoice row={row} label={label} onChange={onChange} />
        </>
      ) : null}

      {row.kind === 'external' ? (
        catalog ? (
          <>
            <label className="flex flex-col gap-1">
              Framework
              <select
                aria-label={label('framework')}
                className={FIELD}
                value={row.adapter}
                onChange={(event) =>
                  onChange({ adapter: event.target.value, frameworkMetric: '', parameters: {} })
                }
              >
                <option value="">Choose…</option>
                {catalog.catalog.adapters.map((entry) => (
                  <option key={entry.name} value={entry.name}>
                    {entry.name} {entry.version}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1">
              Its metric
              <select
                aria-label={label('framework metric')}
                className={FIELD}
                value={row.frameworkMetric}
                disabled={!adapter}
                onChange={(event) =>
                  onChange({
                    frameworkMetric: event.target.value,
                    parameters: {},
                    showsInput: true,
                    inputPath: '',
                  })
                }
              >
                <option value="">Choose…</option>
                {(adapter?.metrics ?? []).map((entry) => (
                  <option key={entry.metric} value={entry.metric}>
                    {entry.metric}
                  </option>
                ))}
              </select>
            </label>
            {offered ? (
              <p className="text-muted-foreground md:col-span-3">
                {offered.description ? `${offered.description} ` : ''}
                {offered.unit}, {offered.direction} is better, folded as a {offered.aggregation};
                reads {offered.reads.join(', ')}
                {offered.model_graded && adapter?.model
                  ? `; graded by ${adapter.model.name} @ ${adapter.model.version}`
                  : ''}
                .
              </p>
            ) : null}
            {Object.entries(offered?.parameters ?? {}).map(([parameterName, parameter]) =>
              parameter.kind === 'boolean' ? (
                <label key={parameterName} className="flex flex-col gap-1">
                  {parameterName}
                  {parameter.required ? ' (required)' : ''}
                  <select
                    aria-label={label(`parameter ${parameterName}`)}
                    className={FIELD}
                    value={row.parameters[parameterName] ?? ''}
                    onChange={(event) =>
                      onChange({
                        parameters: { ...row.parameters, [parameterName]: event.target.value },
                      })
                    }
                  >
                    <option value="">the framework&apos;s default</option>
                    <option value="true">true</option>
                    <option value="false">false</option>
                  </select>
                </label>
              ) : (
                <TextField
                  key={parameterName}
                  label={label(`parameter ${parameterName}`)}
                  title={`${parameterName}${parameter.required ? ' (required)' : ''}${
                    parameter.kind === 'string_list' ? ', comma-separated' : ''
                  }`}
                  value={row.parameters[parameterName] ?? ''}
                  onChange={(value) =>
                    onChange({ parameters: { ...row.parameters, [parameterName]: value } })
                  }
                />
              ),
            )}
            {reads?.includes('input') ? (
              <TextField
                label={label('input path')}
                title="What it reads of the case's input"
                value={row.inputPath}
                placeholder="the whole input"
                onChange={(inputPath) => onChange({ inputPath })}
              />
            ) : null}
            {offered?.model_graded ? (
              <div className="grid gap-2 md:col-span-3 md:grid-cols-3">
                <label className="flex items-center gap-2 md:col-span-3">
                  <input
                    type="checkbox"
                    aria-label={label('held against people')}
                    checked={row.calibrate}
                    onChange={(event) => onChange({ calibrate: event.target.checked })}
                  />
                  Hold its verdicts against people&apos;s judgements — a run then names a
                  calibration set, and the result carries how often the two agreed.
                </label>
                {row.calibrate ? (
                  <>
                    <RubricChoice
                      label={label('calibration rubric')}
                      title="Rubric people judged under"
                      rubrics={rubrics}
                      value={row.calibrationRubric}
                      onChange={(calibrationRubric) =>
                        onChange({ calibrationRubric, calibrationLevel: '', calibrationScore: '' })
                      }
                    />
                    <TextField
                      label={label('passes at')}
                      title={`Passes at (${offered.direction === 'lower' ? 'or below' : 'or above'})`}
                      value={row.passAt}
                      onChange={(passAt) => onChange({ passAt })}
                    />
                    <LevelChoice
                      label={label('person passes at')}
                      title="A person's judgement passes at"
                      rubric={row.calibrationRubric}
                      value={row.calibrationLevel}
                      onChange={(calibrationLevel) => onChange({ calibrationLevel })}
                      none="its better answer"
                      score={{
                        value: row.calibrationScore,
                        onChange: (calibrationScore) => onChange({ calibrationScore }),
                      }}
                    />
                  </>
                ) : null}
              </div>
            ) : null}
          </>
        ) : (
          <p className="text-muted-foreground md:col-span-2">
            {catalogFailed instanceof Error
              ? `Could not read the scorer service's catalog: ${catalogFailed.message}`
              : 'No scorer service has described its metrics to this deployment (AIWATCHER_SCORER_URL, on the process that claims its runs), so no framework metric can be named yet.'}
          </p>
        )
      ) : null}
      {onRemove ? (
        <div className="md:col-span-3">
          <Button size="sm" type="button" variant="outline" onClick={onRemove}>
            Remove scorer {index + 1}
          </Button>
        </div>
      ) : null}
    </fieldset>
  );
}

function TextField({
  label,
  title,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  title: string;
  value: string;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="flex flex-col gap-1">
      {title}
      <input
        aria-label={label}
        className={FIELD}
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

function RubricChoice({
  label,
  title,
  rubrics,
  value,
  onChange,
}: {
  label: string;
  title: string;
  rubrics: RubricHead[];
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="flex flex-col gap-1">
      {title}
      <select
        aria-label={label}
        className={FIELD}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">Choose…</option>
        {rubrics.map((rubric) => (
          <option key={rubric.name} value={`${rubric.name}@${rubric.version}`}>
            {rubric.name} · {pinchId(rubric.version, 8, 6)}
          </option>
        ))}
      </select>
    </label>
  );
}

/** A rubric's named levels, read from the version chosen; nothing to choose on another scale. */
function LevelChoice({
  label,
  title,
  rubric,
  value,
  onChange,
  none,
  score,
}: {
  label: string;
  title: string;
  rubric: string;
  value: string;
  onChange: (value: string) => void;
  none: string;
  /** Where a numeric rubric passes, for a calibration; a judge's has no bar there. */
  score?: { value: string; onChange: (value: string) => void };
}) {
  const pinned = rubric ? reference(rubric) : undefined;
  const read = useQuery({
    queryKey: ['evaluation-rubric', pinned?.name, pinned?.version],
    enabled: Boolean(pinned),
    queryFn: async () =>
      answerOf(
        await getRubric({
          path: { name: pinned?.name ?? '' },
          query: { version: pinned?.version },
        }),
        'could not read this rubric',
      ),
    retry: false,
  });
  const scale = read.data?.rubric.scale;
  if (scale?.kind === 'numeric' && score) {
    const side = read.data?.rubric.direction === 'lower' ? 'or below' : 'or above';
    return (
      <TextField
        label={label}
        title={`${title} (${side}, ${scale.min} to ${scale.max})`}
        value={score.value}
        onChange={score.onChange}
      />
    );
  }
  if (!scale || scale.kind !== 'ordinal') return null;
  return (
    <label className="flex flex-col gap-1">
      {title}
      <select
        aria-label={label}
        className={FIELD}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">{none}</option>
        {scale.levels.map((level) => (
          <option key={level} value={level}>
            {level}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Whether a judge is shown the case's question, and where in it. */
function InputChoice({
  row,
  label,
  onChange,
}: {
  row: Row;
  label: (what: string) => string;
  onChange: (patch: Partial<Row>) => void;
}) {
  return (
    <>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          aria-label={label('shown the input')}
          checked={row.showsInput}
          onChange={(event) => onChange({ showsInput: event.target.checked })}
        />
        Shown what the case asked
      </label>
      {row.showsInput ? (
        <TextField
          label={label('input path')}
          title="Where in the input"
          value={row.inputPath}
          placeholder="the whole input"
          onChange={(inputPath) => onChange({ inputPath })}
        />
      ) : null}
    </>
  );
}
