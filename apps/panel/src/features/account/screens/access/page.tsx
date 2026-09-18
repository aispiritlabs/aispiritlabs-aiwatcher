import { getRouteApi } from '@tanstack/react-router';
import * as React from 'react';

import type { IamProjectAccess } from '@/api/generated';
import {
  edgeOf,
  short,
  useAudit,
  useCreateOrganization,
  useOrganizations,
  useProjectAccess,
  useProjects,
} from '@/features/account/iam';
import { GrantForm } from '@/features/account/screens/access/grant-form';
import { History } from '@/features/account/screens/access/history';
import { OrganizationAdmin } from '@/features/account/screens/access/organization-admin';
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
import { useAuthConfig, useCan, useSession } from '@/shared/lib/auth';
import { cn } from '@/shared/lib/utils';

const routeApi = getRouteApi('/account/access');

/**
 * Organizations, projects and the grants that reach them.
 *
 * This is a **test surface for permissions**, not the multi-tenant panel. What
 * it administers is the IAM control plane and the authored registries keyed
 * under a project — prompts, datasets, annotations, training, evaluations,
 * workflow definitions. Runs, spans, metrics and the live stream are still
 * instance-wide, so a project shared from here has its material and no
 * execution history, and the page says so rather than letting somebody find
 * out.
 *
 * There is deliberately no organization switcher in the header. A selector
 * that scoped the whole panel would be an announcement of multi-tenancy the
 * data plane cannot yet keep; the selection lives here, in the URL, where it
 * means "the thing I am administering".
 */
export function AccessPage() {
  const config = useAuthConfig();
  const enabled = config.data?.enabled === true;
  const session = useSession(enabled);
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const organizations = useOrganizations(enabled);
  const projects = useProjects(search.organization);
  const access = useProjectAccess(search.organization, search.project);
  // Whether this caller administers the organization is the server's answer,
  // not a guess: the audit is owner-and-admin only, so its 403 is that answer.
  // A project admin who is merely a member still issues grants — that form is
  // outside this, which is why it is not gated on the same thing.
  const audit = useAudit(search.organization);
  const createOrganization = useCreateOrganization();
  const mayCreateOrganization = useCan('admin');
  const [name, setName] = React.useState('');

  const issuer = session.data?.issuer ?? undefined;

  const select = (organization?: string, project?: string) =>
    void navigate({ search: { organization, project }, replace: true });

  if (!enabled) {
    return (
      <div className="flex max-w-3xl flex-col gap-4">
        <Heading />
        <p className="text-sm text-muted-foreground">
          This instance has no identity provider, so there is nobody for a grant to name.
          Organizations and projects need{' '}
          <span className="font-mono text-xs">AIWATCHER_AUTH_MODE=oidc</span> and the IAM control
          plane beside it.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <Heading />

      <Card>
        <CardHeader>
          <CardTitle>Organizations</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <p className="text-xs text-muted-foreground">
            Only the ones that list you. An instance administrator may create one and is its first
            owner; nobody can be named as owner by somebody else.
          </p>
          {organizations.isPending ? <Spinner /> : null}
          {organizations.isError ? (
            <Refusal error={organizations.error} fallback="organizations could not be read" />
          ) : null}
          {organizations.data?.length === 0 ? (
            <EmptyState
              title="No organization lists you"
              hint="Either nobody has added you to one, or none exists yet."
            />
          ) : null}
          <div className="flex flex-wrap gap-2">
            {organizations.data?.map((organization) => (
              <button
                key={organization.id}
                type="button"
                onClick={() => select(organization.id, undefined)}
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
          {mayCreateOrganization ? (
            <form
              className="flex flex-wrap items-end gap-2 border-t border-border pt-3"
              onSubmit={(event) => {
                event.preventDefault();
                if (name.trim()) createOrganization.mutate(name.trim());
                setName('');
              }}
            >
              <label className="flex flex-col gap-1">
                <span className="text-xs text-muted-foreground">New organization</span>
                <input
                  className="h-9 rounded-md border border-border bg-transparent px-2 text-sm"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="AI Spirit Labs"
                />
              </label>
              <Button
                type="submit"
                size="sm"
                disabled={createOrganization.isPending || !name.trim()}
              >
                Create
              </Button>
              {createOrganization.isError ? (
                <Refusal
                  error={createOrganization.error}
                  fallback="the organization was not created"
                />
              ) : null}
            </form>
          ) : (
            <p className="text-xs text-muted-foreground">
              Creating an organization needs the instance admin role, which comes from a group in
              the identity provider.
            </p>
          )}
        </CardContent>
      </Card>

      {search.organization ? (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Projects you hold a grant on</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <p className="text-xs text-muted-foreground">
                Membership is not on this list; grants are. An owner sees a project only if somebody
                granted it to them — including the one they created, which grants them explicitly.
              </p>
              {projects.isPending ? <Spinner /> : null}
              {projects.isError ? (
                <Refusal error={projects.error} fallback="projects could not be read" />
              ) : null}
              {projects.data?.length === 0 ? (
                <EmptyState
                  title="No project here reaches you"
                  hint="Create one, or ask an administrator of this organization for a grant."
                />
              ) : null}
              <div className="flex flex-wrap gap-2">
                {projects.data?.map((entry) => (
                  <button
                    key={entry.project.scope.project}
                    type="button"
                    onClick={() => select(search.organization, entry.project.scope.project)}
                    className={cn(
                      'flex flex-col gap-1 rounded-md border border-border px-3 py-2 text-left text-sm transition-colors hover:bg-accent',
                      search.project === entry.project.scope.project && 'border-primary bg-accent',
                    )}
                  >
                    <span className="font-medium">{entry.project.name}</span>
                    <span className="flex items-center gap-2">
                      <Badge tone="primary">{entry.role}</Badge>
                      <span className="text-xs text-muted-foreground">
                        {entry.grants.length} live source{entry.grants.length === 1 ? '' : 's'}
                      </span>
                    </span>
                  </button>
                ))}
              </div>
            </CardContent>
          </Card>

          {search.project ? (
            <>
              <ProjectAccessCard
                access={access.data}
                pending={access.isPending}
                error={access.isError ? access.error : null}
              />
              {access.data?.role === 'admin' || audit.isSuccess ? (
                <GrantForm
                  organization={search.organization}
                  project={search.project}
                  issuer={issuer}
                />
              ) : access.data ? (
                <p className="text-xs text-muted-foreground">
                  Sharing this project needs project admin, or admin of the organization. Yours is{' '}
                  {access.data.role}. This hides a control the server would refuse; it is not the
                  check.
                </p>
              ) : null}
            </>
          ) : null}

          {audit.isSuccess ? (
            <OrganizationAdmin organization={search.organization} issuer={issuer} />
          ) : null}
          <History organization={search.organization} />
        </>
      ) : null}
    </div>
  );
}

function Heading() {
  return (
    <div>
      <h1 className="text-lg font-semibold">Organizations &amp; projects</h1>
      <p className="max-w-3xl text-sm text-muted-foreground">
        Who may see a project is aiwatcher&rsquo;s question, not the identity provider&rsquo;s. The
        provider says who you are; membership, teams and grants are kept here. Identity-provider
        groups decide your instance role and are <span className="font-medium">never</span> teams.
      </p>
      <p className="mt-2 max-w-3xl text-xs text-muted-foreground">
        What a grant reaches today: the authored registries — prompts, datasets, annotations,
        training runs and models, evaluations, workflow definitions, case reviews. Runs, spans,
        metrics and the live stream are still instance-wide, so a shared project has its material
        and not its execution history.
      </p>
    </div>
  );
}

/**
 * One fresh decision, and every source behind it.
 *
 * The role is the maximum over the live grants, which is why each is shown
 * separately: a workshop grant expiring must not read as access being gone
 * when a permanent one is still there.
 */
function ProjectAccessCard({
  access,
  pending,
  error,
}: {
  access: IamProjectAccess | undefined;
  pending: boolean;
  error: unknown;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Your access to this project</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        {pending ? <Spinner /> : null}
        {error ? <Refusal error={error} fallback="this project could not be read" /> : null}
        {access ? (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone="primary">{access.role}</Badge>
              <span className="text-xs text-muted-foreground">
                decided {new Date(access.evaluated_at * 1000).toLocaleString()} — a snapshot, not a
                key: every operation asks again
              </span>
            </div>
            <ul className="flex flex-col gap-2">
              {access.grants.map((entry) => (
                <li
                  key={entry.grant.id}
                  className="flex flex-col gap-1 rounded-md border border-border p-3 text-xs"
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge>{entry.role}</Badge>
                    {entry.role !== entry.grant.role ? (
                      <span className="text-muted-foreground">
                        issued as {entry.grant.role}; its editing period has ended
                      </span>
                    ) : null}
                    <IdChip label="grant" value={short(entry.grant.id)} full={entry.grant.id} />
                  </div>
                  <div className="text-muted-foreground">
                    from {edgeOf(entry.grant.window.valid_from, 'always')}, editing until{' '}
                    {edgeOf(entry.grant.window.edit_until, 'no end')}, reading until{' '}
                    {edgeOf(entry.grant.window.read_until, 'no end')}
                  </div>
                  <div className="text-muted-foreground">
                    {entry.grant.grantee.kind === 'user' ? (
                      <>
                        to{' '}
                        <IdChip
                          label="subject"
                          value={short(entry.grant.grantee.value.subject)}
                          full={entry.grant.grantee.value.subject}
                        />{' '}
                        at{' '}
                        <span className="break-all font-mono">
                          {entry.grant.grantee.value.provider}
                        </span>
                      </>
                    ) : (
                      <>
                        through the team{' '}
                        <IdChip
                          label="team"
                          value={short(entry.grant.grantee.value)}
                          full={entry.grant.grantee.value}
                        />
                      </>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}
