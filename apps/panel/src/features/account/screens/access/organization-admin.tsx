import * as React from 'react';

import type { IamCommand, IamOrganizationRole } from '@/api/generated';
import { useCommand } from '@/features/account/iam';
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Refusal,
} from '@/shared/components/ui/primitives';

/**
 * The organization's own membership, and the teams a grant can name.
 *
 * Membership is not access: an owner reading this has no grant on any project
 * they did not create, and adding somebody here gives them nothing except the
 * ability to be granted something. That is the rule this panel must not blur,
 * so the two live in separate cards with separate sentences.
 *
 * Every id here is typed rather than chosen from a list, because the API has
 * no route that lists members or teams. The History card below is where those
 * ids come from.
 */
export function OrganizationAdmin({
  organization,
  issuer,
}: {
  organization: string;
  issuer: string | undefined;
}) {
  const command = useCommand(organization);
  const [project, setProject] = React.useState('');
  const [team, setTeam] = React.useState('');
  const [teamId, setTeamId] = React.useState('');
  const [memberProvider, setMemberProvider] = React.useState(issuer ?? '');
  const [memberSubject, setMemberSubject] = React.useState('');
  const [memberRole, setMemberRole] = React.useState<IamOrganizationRole>('member');

  // The reader's own issuer, once the session answers. Typing wins: an instance
  // can hold principals from a provider it has since stopped using.
  React.useEffect(() => {
    setMemberProvider((held) => (held === '' && issuer ? issuer : held));
  }, [issuer]);

  function send(next: IamCommand) {
    command.mutate(next);
  }

  return (
    <div className="grid gap-3 lg:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle>Projects and teams</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4 text-sm">
          <form
            className="flex flex-col gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              if (project.trim()) send({ type: 'create_project', name: project.trim() });
              setProject('');
            }}
          >
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">New project</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={project}
                onChange={(event) => setProject(event.target.value)}
                placeholder="Lesson one"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              Creating one grants its creator project admin, explicitly and removably. It is the
              only grant anybody gets without being given one.
            </p>
            <Button type="submit" size="sm" disabled={command.isPending || !project.trim()}>
              Create project
            </Button>
          </form>

          <form
            className="flex flex-col gap-2 border-t border-border pt-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (team.trim()) send({ type: 'create_team', name: team.trim() });
              setTeam('');
            }}
          >
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">New team</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={team}
                onChange={(event) => setTeam(event.target.value)}
                placeholder="Tuesday group"
              />
            </label>
            <p className="text-xs text-muted-foreground">
              A team is aiwatcher&rsquo;s own. Groups from the identity provider are never copied
              into one — they decide your instance role and nothing about a project.
            </p>
            <Button
              type="submit"
              size="sm"
              variant="outline"
              disabled={command.isPending || !team.trim()}
            >
              Create team
            </Button>
          </form>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>People in this organization</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-sm">
          <p className="text-xs text-muted-foreground">
            Membership alone gives no project access. Somebody has to be a member before a grant can
            name them, and their subject is the value their own Profile page shows.
          </p>
          <div className="flex flex-col gap-2">
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Provider</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
                value={memberProvider}
                onChange={(event) => setMemberProvider(event.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Subject</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
                value={memberSubject}
                onChange={(event) => setMemberSubject(event.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Organization role</span>
              <select
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={memberRole}
                onChange={(event) => setMemberRole(event.target.value as IamOrganizationRole)}
              >
                <option value="member">member</option>
                <option value="admin">admin</option>
                <option value="owner">owner</option>
              </select>
            </label>
            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                disabled={command.isPending || !memberSubject.trim() || !memberProvider.trim()}
                onClick={() =>
                  send({
                    type: 'set_member',
                    principal: { provider: memberProvider.trim(), subject: memberSubject.trim() },
                    role: memberRole,
                  })
                }
              >
                Add or change
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={command.isPending || !memberSubject.trim() || !memberProvider.trim()}
                onClick={() =>
                  send({
                    type: 'remove_member',
                    principal: { provider: memberProvider.trim(), subject: memberSubject.trim() },
                  })
                }
              >
                Remove
              </Button>
            </div>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Team id, to put them in one</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
                value={teamId}
                onChange={(event) => setTeamId(event.target.value)}
              />
            </label>
            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={command.isPending || !teamId.trim() || !memberSubject.trim()}
                onClick={() =>
                  send({
                    type: 'set_team_member',
                    team: teamId.trim(),
                    principal: { provider: memberProvider.trim(), subject: memberSubject.trim() },
                    present: true,
                  })
                }
              >
                Add to team
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={command.isPending || !teamId.trim() || !memberSubject.trim()}
                onClick={() =>
                  send({
                    type: 'set_team_member',
                    team: teamId.trim(),
                    principal: { provider: memberProvider.trim(), subject: memberSubject.trim() },
                    present: false,
                  })
                }
              >
                Take out of team
              </Button>
            </div>
          </div>
          <p className="text-xs text-muted-foreground">
            Removing somebody removes their direct grants and team memberships. Adding them again
            does not bring those back.
          </p>
          {command.isError ? (
            <Refusal error={command.error} fallback="the command was not applied" />
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}
