import { getRouteApi } from '@tanstack/react-router';

import type { IamProject, IamProjectAccess } from '@/api/generated';
import { PHASES, phaseOf, windowSentence } from '@/features/learning/lib/enrollment';
import { Labs } from '@/features/learning/screens/overview/labs';
import { Participants } from '@/features/learning/screens/overview/participants';
import { Invitations } from '@/shared/components/invitations';
import { Redeem } from '@/shared/components/redeem-invitation';
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
import { useAuthConfig } from '@/shared/lib/auth';
import {
  edgeOf,
  short,
  useOrganizations,
  useProjectAccess,
  useProjects,
  useRoster,
} from '@/shared/lib/iam';
import { cn } from '@/shared/lib/utils';

const routeApi = getRouteApi('/learning');

/**
 * Workshops, participants and labs — over the control plane that already
 * exists.
 *
 * **A workshop is a project, a participant is a grant, and enrolling is
 * redeeming an invitation.** Nothing in the backend has the word "workshop" in
 * it, and nothing here asks it to: a lesson that runs from Monday to Friday and
 * stays readable afterwards is a `GrantWindow` with a `valid_from`, an
 * `edit_until` and a `read_until`, which is the primitive this area is built on
 * rather than a thing to be built.
 *
 * What that buys is a whole half of this area working on day one: who is on a
 * workshop, when their access opens, when they stop being able to change
 * anything, when it closes, and how somebody joins who has never signed in
 * here. What it does not buy is the other half — a brief to read, tests, a
 * mark. Those have no contract anywhere in this instance and are drawn as
 * empty slots, because `AreaPlaceholder`'s rule holds here too: a plausible
 * fake reads as working software.
 *
 * One more limit is stated on the page rather than left to be discovered. A
 * grant reaches the **authored** registries — prompts, datasets, annotations,
 * training, evaluations, workflow definitions. Runs, spans, metrics and the
 * live stream are still instance-wide, so a workshop has its material and not
 * its execution history until the data plane follows.
 */
export function LearningPage() {
  const config = useAuthConfig();
  const enabled = config.data?.enabled === true;
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const organizations = useOrganizations(enabled);
  const projects = useProjects(search.organization);
  const roster = useRoster(search.organization);
  const access = useProjectAccess(search.organization, search.project);
  // One clock for the page, so every row on it is read against the same
  // moment. Passed down rather than taken per row: two badges disagreeing
  // because a render crossed a boundary is a bug nobody would reproduce.
  const now = Math.floor(Date.now() / 1000);

  const open = (organization?: string, project?: string) =>
    void navigate({ search: { organization, project }, replace: true });

  if (!enabled) {
    return (
      <div className="flex max-w-3xl flex-col gap-4">
        <Heading />
        <Card>
          <CardHeader>
            <CardTitle>This instance has no identity provider</CardTitle>
          </CardHeader>
          <CardContent className="text-sm text-muted-foreground">
            A workshop is a project and a participant is a grant, and a grant names one person at
            one provider. Without{' '}
            <span className="font-mono text-xs">AIWATCHER_AUTH_MODE=oidc</span> and the IAM control
            plane beside it there is nobody for one to name, so there is nothing to enrol anybody
            in.
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <Heading />

      <Redeem
        title="Join a workshop"
        intro={
          <>
            An instructor sends a token. Paste it here and it becomes your place on their workshop —
            once, for whoever is signed in, so redeem it as yourself.
          </>
        }
        onRedeemed={(organization, project) => open(organization, project)}
      />

      <Card>
        <CardHeader>
          <CardTitle>Organizations</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {organizations.isPending ? <Spinner /> : null}
          {organizations.isError ? (
            <Refusal error={organizations.error} fallback="organizations could not be read" />
          ) : null}
          {organizations.data?.length === 0 ? (
            <EmptyState
              title="No organization lists you"
              hint="Redeem an invitation above, or ask whoever runs the workshop to send you one."
            />
          ) : null}
          <div className="flex flex-wrap gap-2">
            {organizations.data?.map((organization) => (
              <button
                key={organization.id}
                type="button"
                onClick={() => open(organization.id, undefined)}
                className={cn(
                  'rounded-md border border-border px-3 py-2 text-left text-sm transition-colors hover:bg-accent',
                  search.organization === organization.id && 'border-primary bg-accent',
                )}
              >
                <span className="block font-medium">{organization.name}</span>
                <span className="id block text-xs text-muted-foreground">
                  {short(organization.id, 18)}
                </span>
              </button>
            ))}
          </div>
        </CardContent>
      </Card>

      {search.organization && !search.project ? (
        <Workshops
          held={projects.data}
          all={roster.data?.projects}
          administered={roster.isSuccess}
          pending={projects.isPending}
          error={projects.isError ? projects.error : null}
          now={now}
          onOpen={(project) => open(search.organization, project)}
        />
      ) : null}

      {search.organization && search.project ? (
        <Workshop
          organization={search.organization}
          project={search.project}
          access={access.data}
          pending={access.isPending}
          error={access.isError ? access.error : null}
          administered={roster.isSuccess}
          now={now}
          onBack={() => open(search.organization, undefined)}
        />
      ) : null}
    </div>
  );
}

function Heading() {
  return (
    <div>
      <h1 className="text-lg font-semibold">Learning</h1>
      <p className="max-w-3xl text-sm text-muted-foreground">
        A workshop is a project, a participant is a grant on it, and enrolling is redeeming an
        invitation. The dates on that grant are the timetable: when access opens, when editing
        stops, when reading stops.
      </p>
      <p className="mt-2 max-w-3xl text-xs text-muted-foreground">
        What a participant&rsquo;s grant reaches is the authored material — prompts, datasets,
        annotations, training runs and models, evaluations, workflow definitions. Runs, spans,
        metrics and the live stream are still instance-wide, so a workshop has its material and not
        its execution history.
      </p>
    </div>
  );
}

/**
 * Every workshop in the organization: the ones open to you now, then the rest.
 *
 * Two reads, kept apart because they answer different questions. `projects` is
 * what the caller holds a **live** grant on, with the server's own verdict on
 * each; a roster's `projects` is every project in the organization and only an
 * organization administrator gets one — which is exactly the instructor who may
 * enrol somebody on a workshop they are not themselves on.
 *
 * "Live" is the word that decides the headings, and the browser check is what
 * found it. `Policy::projects` keeps a project only where `access` succeeds, and
 * `access` needs a grant in force *now* — so a workshop somebody is enrolled on
 * from next Monday is not on the first list, and one whose reading has ended is
 * not either. The second list therefore cannot be called "the ones you only
 * administer": for an organization admin it holds all three cases at once, and
 * for a participant with a future place it holds nothing at all, because the
 * roster is not theirs to read. Both are said in words rather than guessed at,
 * and the workshop's own page is where the grants that explain it are.
 */
function Workshops({
  held,
  all,
  administered,
  pending,
  error,
  now,
  onOpen,
}: {
  held: IamProjectAccess[] | undefined;
  all: IamProject[] | undefined;
  /** Whether the roster answered, which is the server saying this caller administers it. */
  administered: boolean;
  pending: boolean;
  error: unknown;
  now: number;
  onOpen: (project: string) => void;
}) {
  const mine = held ?? [];
  const known = new Set(mine.map((entry) => entry.project.scope.project));
  const rest = (all ?? []).filter((project) => !known.has(project.scope.project));

  return (
    <Card>
      <CardHeader>
        <CardTitle>Workshops</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {pending ? <Spinner /> : null}
        {error ? <Refusal error={error} fallback="workshops could not be read" /> : null}
        {!pending && mine.length === 0 && rest.length === 0 ? (
          <EmptyState
            title="No workshop here is open to you"
            hint="A grant in force is what puts one on this list, so a place that starts later is not here yet and one that has closed has left it. Redeem an invitation, or ask an administrator of this organization."
          />
        ) : null}

        {!administered ? (
          // Without the roster this list is only what a live grant reaches, and
          // the workshops that are missing from it are exactly the ones a
          // participant would most want told about: the one starting Monday.
          // Saying so is the whole fix available here — there is no route that
          // answers "what am I enrolled on", only "what may I reach now".
          <p className="text-xs text-muted-foreground">
            This is what a grant of yours reaches <span className="font-medium">now</span>. A place
            that opens later is not here yet and one that has closed has left, because the server
            answers with access rather than with enrolment. An administrator of this organization
            sees the rest.
          </p>
        ) : null}

        {mine.length > 0 ? (
          <section className="flex flex-col gap-2">
            <h2 className="text-xs font-medium text-muted-foreground">Open to you now</h2>
            <ul className="flex flex-col gap-2">
              {mine.map((entry) => (
                <li key={entry.project.scope.project}>
                  <button
                    type="button"
                    onClick={() => onOpen(entry.project.scope.project)}
                    className="flex w-full flex-col gap-1 rounded-md border border-border p-3 text-left transition-colors hover:bg-accent"
                  >
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="text-sm font-medium">{entry.project.name}</span>
                      {/* The server's verdict for the whole project, not a
                          reading of any one window: `role` is the maximum over
                          the live sources and only it knows about all of them. */}
                      <Badge tone="primary">{entry.role}</Badge>
                      <span className="text-xs text-muted-foreground">
                        {entry.grants.length} live source{entry.grants.length === 1 ? '' : 's'}
                      </span>
                    </span>
                    <span className="flex flex-col gap-0.5 text-xs text-muted-foreground">
                      {entry.grants.map((source) => {
                        const phase = phaseOf(source.grant.window, source.grant.role, now);
                        return (
                          <span key={source.grant.id}>
                            {PHASES[phase].label} — {windowSentence(source.grant.window, phase)}
                          </span>
                        );
                      })}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {rest.length > 0 ? (
          <section className="flex flex-col gap-2">
            <h2 className="text-xs font-medium text-muted-foreground">Also in this organization</h2>
            <p className="text-xs text-muted-foreground">
              No grant of yours is in force on these, and there are three ways that happens: you
              administer the organization and were never enrolled, your place has not opened yet, or
              it has closed. They carry no role because there is nothing to report about access
              nobody has right now. Open one to see the grants that say which — enrolling somebody
              works from there either way, and opening the material does not.
            </p>
            <ul className="flex flex-col gap-2">
              {rest.map((project) => (
                <li key={project.scope.project}>
                  <button
                    type="button"
                    onClick={() => onOpen(project.scope.project)}
                    className="flex w-full flex-wrap items-center gap-2 rounded-md border border-border p-3 text-left text-sm transition-colors hover:bg-accent"
                  >
                    <span className="font-medium">{project.name}</span>
                    <span className="text-xs text-muted-foreground">nothing of yours in force</span>
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ) : null}
      </CardContent>
    </Card>
  );
}

/** One workshop: your place on it, who else is, how somebody joins, and the labs. */
function Workshop({
  organization,
  project,
  access,
  pending,
  error,
  administered,
  now,
  onBack,
}: {
  organization: string;
  project: string;
  access: IamProjectAccess | null | undefined;
  pending: boolean;
  error: unknown;
  /** Whether the roster answered, which is the server saying this caller administers it. */
  administered: boolean;
  now: number;
  onBack: () => void;
}) {
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>{access?.project.name ?? 'This workshop'}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="outline" onClick={onBack}>
              All workshops
            </Button>
            <IdChip label="project" value={short(project, 18)} full={project} />
          </div>
          {pending ? <Spinner /> : null}
          {error ? <Refusal error={error} fallback="this workshop could not be read" /> : null}
          {access === null ? (
            <p className="text-xs text-muted-foreground">
              You hold no grant on this workshop, so there is no place of yours to report. An
              administrator may enrol people here without being on it themselves.
            </p>
          ) : null}
          {access ? (
            <>
              <div className="flex flex-wrap items-center gap-2">
                <Badge tone="primary">{access.role}</Badge>
                <span className="text-xs text-muted-foreground">
                  decided {new Date(access.evaluated_at * 1000).toLocaleString()} — a snapshot, not
                  a key: every operation asks again
                </span>
              </div>
              <ul className="flex flex-col gap-2">
                {access.grants.map((source) => {
                  const phase = phaseOf(source.grant.window, source.grant.role, now);
                  return (
                    <li
                      key={source.grant.id}
                      className="flex flex-col gap-1 rounded-md border border-border p-3 text-xs"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge tone={PHASES[phase].tone}>{PHASES[phase].label}</Badge>
                        {/* Two roles, and they differ on purpose: the second
                            is what the server says this source grants now,
                            the first is what it was issued as. */}
                        <Badge>{source.role}</Badge>
                        {source.role !== source.grant.role ? (
                          <span className="text-muted-foreground">
                            issued as {source.grant.role}; its editing period has ended
                          </span>
                        ) : null}
                        <IdChip
                          label="grant"
                          value={short(source.grant.id)}
                          full={source.grant.id}
                        />
                      </div>
                      <div className="text-muted-foreground">
                        {windowSentence(source.grant.window, phase)}, from{' '}
                        {edgeOf(source.grant.window.valid_from, 'always')}
                      </div>
                    </li>
                  );
                })}
              </ul>
            </>
          ) : null}
        </CardContent>
      </Card>

      <Participants organization={organization} project={project} now={now} />

      {access?.role === 'admin' || administered ? (
        <Invitations
          organization={organization}
          project={project}
          title="Enrol somebody"
          intro={
            <>
              For somebody who has not signed in here yet, so there is no subject to grant to. Send
              them the token; they paste it on their own Learning page and arrive with exactly the
              window declared here. The role is what they get on the workshop&rsquo;s material.
            </>
          }
        />
      ) : access ? (
        <p className="text-xs text-muted-foreground">
          Enrolling somebody needs admin on this workshop, or admin of the organization. Yours is{' '}
          {access.role}. This hides a control the server would refuse; it is not the check.
        </p>
      ) : null}

      <Labs />
    </div>
  );
}
