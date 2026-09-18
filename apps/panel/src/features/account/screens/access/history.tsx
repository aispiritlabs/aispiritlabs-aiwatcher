import type { AuditEntry, IamChange, IamCommand, IamGrant, IamGrantee } from '@/api/generated';
import { edgeOf, short, useAudit, useCommand } from '@/features/account/iam';
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
 * What this organization's administrators have done, newest first.
 *
 * **A history of mutations, not the current state.** A grant that has since
 * been revoked still appears here, with the window it was issued with, because
 * that is what happened — and a revocation appears beside it. Who may reach a
 * project *now* is the grants list on the project, and what any one person may
 * do is an access decision the server takes fresh.
 *
 * It keeps the Revoke action because a grant is named here the moment it is
 * issued, which is occasionally faster than finding it again in the list.
 */
export function History({ organization }: { organization: string }) {
  const entries = useAudit(organization);
  const command = useCommand(organization);

  const forbidden = entries.error instanceof ApiFailure && entries.error.status === 403;

  return (
    <Card>
      <CardHeader>
        <CardTitle>History</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <p className="text-xs text-muted-foreground">
          Successful administrative changes, as the server recorded them in the same transaction —
          including the ones that were later undone. Who has access now is the project&rsquo;s own
          grants list.
        </p>
        {entries.isPending ? <Spinner /> : null}
        {forbidden ? (
          <p className="text-xs text-muted-foreground">
            The history is for this organization&rsquo;s owners and admins. You are a member of it.
          </p>
        ) : entries.isError ? (
          <Refusal error={entries.error} fallback="the audit could not be read" />
        ) : null}
        {entries.data && entries.data.length === 0 ? (
          <EmptyState title="Nothing has been changed yet" />
        ) : null}
        <ul className="flex flex-col gap-2">
          {[...(entries.data ?? [])].reverse().map((entry) => (
            <li
              key={entry.sequence}
              className="flex flex-col gap-1 rounded-md border border-border p-3 text-sm"
            >
              <div className="flex flex-wrap items-center gap-2">
                <Badge>{new Date(entry.occurred_at * 1000).toLocaleString()}</Badge>
                <span>{sentenceOf(entry)}</span>
              </div>
              <div className="text-xs text-muted-foreground">
                by{' '}
                <IdChip
                  label="subject"
                  value={short(entry.actor.subject)}
                  full={entry.actor.subject}
                />
              </div>
              {grantOf(entry.action) ? (
                <GrantLine
                  grant={grantOf(entry.action) as IamGrant}
                  disabled={command.isPending}
                  onRevoke={(grant) =>
                    command.mutate({
                      type: 'revoke_grant',
                      project: grant.scope.project,
                      grant: grant.id,
                    })
                  }
                />
              ) : null}
            </li>
          ))}
        </ul>
        {command.isError ? (
          <Refusal error={command.error} fallback="the command was not applied" />
        ) : null}
      </CardContent>
    </Card>
  );
}

function GrantLine({
  grant,
  disabled,
  onRevoke,
}: {
  grant: IamGrant;
  disabled: boolean;
  onRevoke: (grant: IamGrant) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
      <IdChip label="grant" value={short(grant.id)} full={grant.id} />
      <span>
        {granteeOf(grant.grantee)} as {grant.role}, from {edgeOf(grant.window.valid_from, 'always')}
        , editing until {edgeOf(grant.window.edit_until, 'no end')}, reading until{' '}
        {edgeOf(grant.window.read_until, 'no end')}
      </span>
      <Button variant="outline" size="sm" disabled={disabled} onClick={() => onRevoke(grant)}>
        Revoke
      </Button>
    </div>
  );
}

function grantOf(action: AuditEntry['action']): IamGrant | null {
  if (action.type !== 'command_applied') return null;
  return changeGrant(action.change);
}

function changeGrant(change: IamChange): IamGrant | null {
  return typeof change === 'object' && 'GrantCreated' in change ? change.GrantCreated : null;
}

function granteeOf(grantee: IamGrantee): string {
  return grantee.kind === 'user' ? short(grantee.value.subject) : `team ${short(grantee.value)}`;
}

function sentenceOf(entry: AuditEntry): string {
  if (entry.action.type === 'organization_created') {
    return `created the organization ${entry.action.organization.name}`;
  }
  return describe(entry.action.command, entry.action.change);
}

function describe(command: IamCommand, change: IamChange): string {
  switch (command.type) {
    case 'set_member':
      return `made ${short(command.principal.subject)} an organization ${command.role}`;
    case 'remove_member':
      return `removed ${short(command.principal.subject)} from the organization`;
    case 'create_team':
      return typeof change === 'object' && 'TeamCreated' in change
        ? `created the team ${change.TeamCreated.name} (${change.TeamCreated.id})`
        : `created the team ${command.name}`;
    case 'delete_team':
      return `deleted the team ${short(command.team)}`;
    case 'set_team_member':
      return command.present
        ? `added ${short(command.principal.subject)} to team ${short(command.team)}`
        : `removed ${short(command.principal.subject)} from team ${short(command.team)}`;
    case 'create_project':
      return typeof change === 'object' && 'ProjectCreated' in change
        ? `created the project ${change.ProjectCreated.name} (${short(change.ProjectCreated.scope.project)})`
        : `created the project ${command.name}`;
    case 'grant':
      return `granted ${command.role} on ${short(command.project)}`;
    case 'revoke_grant':
      return `revoked ${short(command.grant)} on ${short(command.project)}`;
  }
}
