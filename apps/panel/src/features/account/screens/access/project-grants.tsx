import type { IamGrant } from '@/api/generated';
import { edgeOf, short, useCommand, useProjectGrants } from '@/shared/lib/iam';
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
import { ApiFailure } from '@/shared/lib/result';

/**
 * Who may reach this project, and until when.
 *
 * Every grant as it was **issued**, including the ones that have lapsed — a
 * lapsed row is usually the thing somebody opened this list to find, and a
 * table quietly filtered by the clock would answer a question nobody asked.
 * What a grant does *now* is an access decision, taken per person, and this
 * page shows exactly one of those: the reader's own.
 *
 * So nothing here works out whether a row is live. It says when it starts, when
 * editing ends and when reading ends, and leaves the arithmetic to the person
 * who knows what today is.
 */
export function ProjectGrants({
  organization,
  project,
}: {
  organization: string;
  project: string;
}) {
  const grants = useProjectGrants(organization, project);
  const command = useCommand(organization);
  const forbidden = grants.error instanceof ApiFailure && grants.error.status === 403;

  return (
    <Card>
      <CardHeader>
        <CardTitle>Who may reach this project</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        {grants.isPending ? <Spinner /> : null}
        {forbidden ? (
          <p className="text-xs text-muted-foreground">
            This list is for whoever may issue a grant here — this project&rsquo;s admin, or an
            admin of the organization.
          </p>
        ) : grants.isError ? (
          <Refusal error={grants.error} fallback="the grants could not be read" />
        ) : null}
        {grants.data?.length === 0 ? <EmptyState title="Nobody has been granted anything" /> : null}
        <ul className="flex flex-col gap-2">
          {grants.data?.map((grant) => (
            <li key={grant.id} className="flex flex-col gap-1 rounded-md border border-border p-3">
              <div className="flex flex-wrap items-center gap-2">
                <Badge>{grant.role}</Badge>
                {grant.grantee.kind === 'user' ? (
                  <IdChip
                    label="subject"
                    value={short(grant.grantee.value.subject, 16)}
                    full={grant.grantee.value.subject}
                  />
                ) : (
                  <span className="flex items-center gap-1 text-xs">
                    team{' '}
                    <IdChip
                      label="team"
                      value={short(grant.grantee.value)}
                      full={grant.grantee.value}
                    />
                  </span>
                )}
                <Button
                  size="sm"
                  variant="outline"
                  className="ml-auto"
                  disabled={command.isPending}
                  onClick={() => command.mutate({ type: 'revoke_grant', project, grant: grant.id })}
                >
                  Revoke
                </Button>
              </div>
              <div className="text-xs text-muted-foreground">
                from {edgeOf(grant.window.valid_from, 'always')}, editing until{' '}
                {edgeOf(grant.window.edit_until, 'no end')}, reading until{' '}
                {edgeOf(grant.window.read_until, 'no end')}
              </div>
              <GrantId grant={grant} />
            </li>
          ))}
        </ul>
        {command.isError ? (
          <Refusal error={command.error} fallback="the grant was not revoked" />
        ) : null}
      </CardContent>
    </Card>
  );
}

function GrantId({ grant }: { grant: IamGrant }) {
  return (
    <div className="text-xs text-muted-foreground">
      <IdChip label="grant" value={short(grant.id)} full={grant.id} />
    </div>
  );
}
