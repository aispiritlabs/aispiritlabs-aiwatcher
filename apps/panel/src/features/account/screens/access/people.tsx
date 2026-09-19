import type { IamMembership, IamPrincipal, IamRoster } from '@/api/generated';
import { short } from '@/shared/lib/iam';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
} from '@/shared/components/ui/primitives';

/** One list of people, whichever standing they hold. */
function Roll({
  people,
  onGrantTo,
}: {
  people: IamMembership[];
  onGrantTo: (principal: IamPrincipal) => void;
}) {
  return (
    <ul className="flex flex-col gap-2">
      {people.map((member) => (
        <li
          key={`${member.principal.provider}\u0000${member.principal.subject}`}
          className="flex flex-wrap items-center gap-2 rounded-md border border-border p-2"
        >
          <Badge tone={member.role === 'admin' || member.role === 'owner' ? 'primary' : 'neutral'}>
            {member.role}
          </Badge>
          <IdChip
            label="subject"
            value={short(member.principal.subject, 16)}
            full={member.principal.subject}
          />
          <span className="min-w-0 break-all font-mono text-xs text-muted-foreground">
            {member.principal.provider}
          </span>
          <Button
            size="sm"
            variant="outline"
            className="ml-auto"
            onClick={() => onGrantTo(member.principal)}
          >
            Grant to…
          </Button>
        </li>
      ))}
    </ul>
  );
}

/**
 * Who is in this organization, and which teams a grant can name.
 *
 * Every role here is what somebody was **given** at the organization level,
 * which reaches no project: an owner appears at the top of this list and may
 * still be unable to open a single project in it. The page keeps the two
 * sentences apart on purpose, because conflating them is the mistake this
 * whole control plane is built to refuse.
 *
 * Read from `GET /iam/organizations/{id}/roster`, which is owner-and-admin
 * only. A member who may reach one project has no business reading the rest of
 * the membership, so this card is simply absent for them.
 */
export function People({
  roster,
  onGrantTo,
}: {
  roster: IamRoster;
  onGrantTo: (principal: IamPrincipal) => void;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>People and teams</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 text-sm">
        <p className="text-xs text-muted-foreground">
          An organization role is not access to anything in it. It decides who may administer the
          organization; reaching a project takes a grant, which is what the button on each row
          starts.
        </p>

        <Roll people={roster.members} onGrantTo={onGrantTo} />

        <div className="flex flex-col gap-2 border-t border-border pt-4">
          <h3 className="text-xs font-medium text-muted-foreground">Guests</h3>
          <p className="text-xs text-muted-foreground">
            Invited to a project and not of this organization: a grant to everybody here does not
            reach them, a team does not take them, and only an explicit change of role makes one a
            member.
          </p>
          {/* Optional in the contract, because a document written before guests
              existed has no list — which reads the same as having none. */}
          {(roster.guests ?? []).length === 0 ? (
            <EmptyState
              title="No guests here"
              hint="An invitation makes one unless it says it wants a colleague."
            />
          ) : (
            <Roll people={roster.guests ?? []} onGrantTo={onGrantTo} />
          )}
        </div>

        <div className="flex flex-col gap-2 border-t border-border pt-4">
          <h3 className="text-xs font-medium text-muted-foreground">Teams</h3>
          {roster.teams.length === 0 ? (
            <EmptyState
              title="No teams here"
              hint="A team is aiwatcher's own; identity-provider groups are never copied into one."
            />
          ) : (
            <ul className="flex flex-col gap-2">
              {roster.teams.map((entry) => (
                <li
                  key={entry.team.id}
                  className="flex flex-col gap-1 rounded-md border border-border p-2"
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{entry.team.name}</span>
                    <IdChip label="team" value={short(entry.team.id)} full={entry.team.id} />
                    <span className="text-xs text-muted-foreground">
                      {entry.members.length} member{entry.members.length === 1 ? '' : 's'}
                    </span>
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {entry.members.map((principal) => (
                      <IdChip
                        key={`${principal.provider}\u0000${principal.subject}`}
                        label="subject"
                        value={short(principal.subject)}
                        full={principal.subject}
                      />
                    ))}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
