import { getRouteApi } from '@tanstack/react-router';

import type {
  DurableEvaluation,
  IamProjectRole,
  LabMeasurementView,
  LabSummary,
  LabVersion,
} from '@/api/generated';
import { useLab, useLabMeasurement, useLabResults, useLabs } from '@/features/learning/lib/labs';
import { ComposeLab } from '@/features/learning/screens/overview/compose';
import { Notebook } from '@/features/learning/screens/overview/notebook';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
  Refusal,
  Spinner,
} from '@/shared/components/ui/primitives';
import { short } from '@/shared/lib/iam';
import { cn } from '@/shared/lib/utils';

const routeApi = getRouteApi('/learning');

/**
 * A workshop's labs, and the one sentence that made them buildable.
 *
 * **A lab is an authored brief bound to a pinned measurement** (ADR_0034), and
 * three of the four things the empty slots used to ask for turned out to exist
 * already under evaluation's names: the tests are a scorecard version and a
 * derived cohort, the work handed in is a recording or answers a worker
 * generated, and a mark is a published evaluation result. So this screen is
 * mostly a *reader* of registries this instance already had.
 *
 * What it must not do is the arithmetic. The `context_id` that joins a lab to
 * its marks is a digest over a canonicalised context, and the server answers
 * it; the direction of each metric is derived by the scorecard, and the server
 * sends it; whether one result is better than another is
 * `evaluation-results/{id}/comparison`, and this screen does not ask. What is
 * drawn here is what came back.
 *
 * The honest absences are still absences. A lab that pins no tests says so in
 * the server's own words rather than showing an empty measurement; a workshop
 * with no labs says that rather than drawing nine placeholder slots; and
 * **starting** a run that scores somebody's submission is still an instance
 * route — ADR_0033 opens no project `/start` until the data plane follows — so
 * that is said in words instead of drawn as a button that would be refused.
 */
export function Labs({
  organization,
  project,
  role,
}: {
  organization: string;
  project: string;
  /** This caller's grant on the workshop. A viewer reads; anything more writes. */
  role: IamProjectRole | undefined;
}) {
  const labs = useLabs(organization, project);
  const opened = useOpenLab(labs.data);

  return (
    <Card>
      <CardHeader>
        <CardTitle>Labs</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <p className="max-w-3xl text-xs text-muted-foreground">
          A lab is a brief and the measurement the work is held to. The tests are a scorecard at a
          version and a cohort of cases; a submission is scored against them and publishes a result,
          and every result measured this way shares one context — which is what makes the marks
          below a list of the same question answered by different people.
        </p>

        {labs.isPending ? <Spinner /> : null}
        {labs.isError ? (
          <Refusal error={labs.error} fallback="this workshop's labs could not be read" />
        ) : null}
        {labs.isSuccess && labs.data.length === 0 ? (
          <EmptyState
            title="This workshop has no labs yet"
            hint="A lab is published to this project like a prompt is: a brief, and the scorecard and cohort it is measured by. Nothing is drawn here until one exists, because a placeholder lab reads as an exercise somebody forgot to write."
          />
        ) : null}

        {role === 'editor' || role === 'admin' ? (
          <ComposeLab
            organization={organization}
            project={project}
            onPublished={(name) => opened.open(name)}
          />
        ) : role ? (
          <p className="text-xs text-muted-foreground">
            Writing a lab needs editor or admin on this workshop. Yours is {role}. This hides a
            control the server would refuse; it is not the check.
          </p>
        ) : null}

        {labs.data && labs.data.length > 0 ? (
          <ol className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {labs.data.map((lab) => (
              <li key={lab.name}>
                <button
                  type="button"
                  onClick={() => opened.open(lab.name)}
                  aria-expanded={opened.name === lab.name}
                  className={cn(
                    'flex w-full flex-col gap-2 rounded-md border border-border p-3 text-left transition-colors hover:bg-accent',
                    opened.name === lab.name && 'border-primary bg-accent',
                  )}
                >
                  <span className="flex flex-wrap items-center gap-2">
                    <span className="text-sm font-medium">{titleOf(lab)}</span>
                    {lab.current?.position != null ? (
                      <Badge>lab {lab.current.position}</Badge>
                    ) : (
                      <Badge tone="warning">unplaced</Badge>
                    )}
                    {lab.is_published ? null : <Badge tone="warning">draft</Badge>}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {lab.current?.has_tests
                      ? 'measured'
                      : 'no tests pinned — the brief is all there is yet'}
                    {' · '}
                    {lab.versions} version{lab.versions === 1 ? '' : 's'}
                  </span>
                </button>
              </li>
            ))}
          </ol>
        ) : null}

        {opened.name ? (
          <Lab
            organization={organization}
            project={project}
            name={opened.name}
            mine={opened.notebook}
            onMine={opened.work}
            onClose={() => opened.open(undefined)}
          />
        ) : null}
      </CardContent>
    </Card>
  );
}

/**
 * Which lab is open, and which copy of its notebook is being worked in.
 *
 * Both in the URL. Which lab was component state while a lab was a section of
 * this page and nothing else — the workshop was already in the search params,
 * and a section is not a view. That stopped being true the day a lab handed
 * out a notebook: opening one runs code and working in one makes a file of
 * your own, and both are things somebody comes back to and sends to a
 * classmate. That is what the plan meant by a lab earning a route.
 *
 * A name no lab in this workshop has opens nothing, so a stale link is an
 * empty selection rather than a request for something that is not there — and
 * changing lab puts the copy away, because a copy belongs to the exercise it
 * was taken from.
 */
function useOpenLab(labs: LabSummary[] | undefined) {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const known = labs?.some((lab) => lab.name === search.lab) ?? false;
  return {
    name: known ? search.lab : undefined,
    notebook: known ? search.notebook : undefined,
    open: (next: string | undefined) =>
      void navigate({
        search: (current) => ({
          ...current,
          lab: current.lab === next ? undefined : next,
          notebook: undefined,
        }),
        replace: true,
      }),
    work: (notebook: string | undefined) =>
      void navigate({ search: (current) => ({ ...current, notebook }), replace: true }),
  };
}

function titleOf(lab: LabSummary): string {
  return lab.current?.title ?? lab.name;
}

/** One lab, opened: the brief, what measures it, and the marks so far. */
function Lab({
  organization,
  project,
  name,
  mine,
  onMine,
  onClose,
}: {
  organization: string;
  project: string;
  name: string;
  mine: string | undefined;
  onMine: (notebook: string | undefined) => void;
  onClose: () => void;
}) {
  const lab = useLab(organization, project, name);
  const measurement = useLabMeasurement(organization, project, name);
  const contextId = measurement.data?.measurement?.context_id;
  const results = useLabResults(organization, project, contextId);

  return (
    <section className="flex flex-col gap-4 rounded-md border border-border p-4">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="text-sm font-medium">{lab.data?.current?.title ?? name}</h2>
        <IdChip label="lab" value={name} />
        {lab.data?.current ? (
          <IdChip
            label="version"
            value={short(lab.data.current.version_id)}
            full={lab.data.current.version_id}
          />
        ) : null}
        <Button size="sm" variant="outline" className="ml-auto" onClick={onClose}>
          Close
        </Button>
      </div>

      {lab.isPending ? <Spinner /> : null}
      {lab.isError ? <Refusal error={lab.error} fallback="this lab could not be read" /> : null}

      <Brief current={lab.data?.current} labelled={Boolean(lab.data?.head.labels.published)} />
      <Notebook pinned={lab.data?.current?.notebook} mine={mine} onMine={onMine} />
      <Tests view={measurement.data} pending={measurement.isPending} error={measurement.error} />
      <Marks
        results={results.data}
        pending={Boolean(contextId) && results.isPending}
        error={results.error}
        contextId={contextId}
        measured={Boolean(measurement.data?.measurement)}
      />
    </section>
  );
}

/**
 * The brief, as it was written.
 *
 * Rendered verbatim, with its line breaks kept and nothing else done to it.
 * This panel has no Markdown renderer and one would be a dependency chosen as
 * a side effect of drawing a lesson; until somebody decides that on purpose,
 * showing the text as written is the honest thing, because interpreting half
 * of a syntax is worse than interpreting none.
 */
function Brief({
  current,
  labelled,
}: {
  current: LabVersion | null | undefined;
  labelled: boolean;
}) {
  if (!current) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-medium text-muted-foreground">Brief</h3>
        {labelled ? null : (
          <Badge tone="warning">
            no <span className="font-mono">published</span> label — this is the newest draft
          </Badge>
        )}
      </div>
      <p className="max-w-3xl text-sm whitespace-pre-wrap">{current.brief}</p>
    </div>
  );
}

/** What measures the work: the card, the cases, and what each metric means. */
function Tests({
  view,
  pending,
  error,
}: {
  view: LabMeasurementView | undefined;
  pending: boolean;
  error: unknown;
}) {
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-xs font-medium text-muted-foreground">Tests</h3>
      {pending ? <Spinner /> : null}
      {error ? <Refusal error={error} fallback="what this lab measures could not be read" /> : null}
      {view?.unavailable ? (
        // The server's own sentence. A lab being written pins no tests yet,
        // which is a fact about the lab rather than a failure — so it is prose
        // and not a red box.
        <p className="max-w-3xl text-xs text-muted-foreground">{view.unavailable}</p>
      ) : null}
      {view?.measurement ? (
        <>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
            <dt className="text-muted-foreground">Scorecard</dt>
            <dd>
              {view.measurement.context.suite.name}
              <span className="text-muted-foreground">
                {' @ '}
                {short(view.measurement.context.suite.version)}
              </span>
            </dd>
            <dt className="text-muted-foreground">Cases</dt>
            <dd>
              {view.measurement.context.case_count} from{' '}
              <span className="font-medium">{view.measurement.context.dataset.name}</span>, split{' '}
              {view.measurement.context.split}
            </dd>
            <dt className="text-muted-foreground">Context</dt>
            <dd className="id">{short(view.measurement.context_id, 24)}</dd>
          </dl>
          <ul className="flex flex-wrap gap-2">
            {view.measurement.context.metrics.map((metric) => (
              <li key={metric.name}>
                <Badge>
                  {metric.name}
                  <span className="ml-1 font-normal text-muted-foreground">
                    {metric.direction === 'higher'
                      ? 'higher is better'
                      : metric.direction === 'lower'
                        ? 'lower is better'
                        : 'no direction'}
                  </span>
                </Badge>
              </li>
            ))}
          </ul>
          <p className="max-w-3xl text-xs text-muted-foreground">
            Which way each number is better comes from the scorecard, never from the lab — a lab
            that restated it would be free to disagree with the card it names. A submission is
            scored by declaring a run against these cases and this card; declaring one is a project
            route, and <span className="font-mono">starting</span> it is still an instance route, so
            that half is done from Evaluation until the data plane follows.
          </p>
        </>
      ) : null}
    </div>
  );
}

/** The marks: every result published on this lab's cases by this lab's card. */
function Marks({
  results,
  pending,
  error,
  contextId,
  measured,
}: {
  results: DurableEvaluation[] | undefined;
  pending: boolean;
  error: unknown;
  contextId: string | undefined;
  measured: boolean;
}) {
  if (!measured) return null;
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-xs font-medium text-muted-foreground">Results</h3>
      {pending ? <Spinner /> : null}
      {error ? <Refusal error={error} fallback="this lab's results could not be read" /> : null}
      {results && results.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          Nothing has been measured on these cases yet. A result appears here when a scoring run
          against this lab&rsquo;s card and cohort publishes one — it shares this lab&rsquo;s
          context by construction, so nothing has to register it.
        </p>
      ) : null}
      {results && results.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {results.map((result) => (
            <li
              key={`${result.receipt.evaluation_id}/${result.receipt.version}`}
              className="flex flex-col gap-1 rounded-md border border-border p-3 text-xs"
            >
              <div className="flex flex-wrap items-center gap-2">
                <IdChip
                  label="variant"
                  value={short(result.receipt.variant_id)}
                  full={result.receipt.variant_id}
                />
                {/* The evidence's own word, uncoloured except where it is
                    whole: "partial" and "expired" are facts about what can
                    still be read, not verdicts about the work. */}
                <Badge tone={result.state === 'complete' ? 'success' : undefined}>
                  {result.state}
                </Badge>
                {result.status ? <Badge>{result.status}</Badge> : null}
                <span className="text-muted-foreground">
                  {new Date(result.receipt.committed_at * 1000).toLocaleString()}
                </span>
              </div>
              <div className="flex flex-wrap gap-x-4 gap-y-1">
                {Object.entries(result.metrics).map(([metric, value]) => (
                  <span key={metric}>
                    <span className="text-muted-foreground">{metric}</span>{' '}
                    <span className="font-medium">{value}</span>
                  </span>
                ))}
              </div>
            </li>
          ))}
        </ul>
      ) : null}
      {contextId ? (
        <p className="text-xs text-muted-foreground">
          These are the results whose context is <span className="id">{short(contextId, 24)}</span>{' '}
          — the same cases measured the same way. Whether one of them is better than another is the
          server&rsquo;s comparison, on the Evaluation page; this list reports what each one
          measured and ranks nothing.
        </p>
      ) : null}
    </div>
  );
}
