import * as React from 'react';

import type { IamCommand, IamPrincipal, IamProjectRole } from '@/api/generated';
import { localInputValue, unixFrom, useCommand } from '@/features/account/iam';
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Refusal,
} from '@/shared/components/ui/primitives';

const ROLES: IamProjectRole[] = ['viewer', 'editor', 'admin'];

/**
 * Sharing a project, which is the whole of sharing a lesson.
 *
 * `valid_from`, `edit_until` and `read_until` are the primitive: access from,
 * editing until, reading until. A workshop that runs on Tuesday and stays
 * readable afterwards is one grant with two of the three filled in, and
 * nothing in the backend needs to know the word "workshop".
 *
 * Three things this form deliberately does not do. It does not look anybody
 * up — a principal is `(provider, subject)` compared exactly, and there is no
 * directory here to search, which is what invitations will be for. It does not
 * pre-fill the other person's provider from anything but the reader's own
 * session, because guessing it would produce a grant that silently matches
 * nobody. And it works nothing out about what the grant will *do*: the window
 * goes to the server and the server answers with a role.
 */
export function GrantForm({
  organization,
  project,
  issuer,
  grantee,
}: {
  organization: string;
  project: string;
  issuer: string | undefined;
  /** Somebody picked out of the roster, so a subject is never retyped. */
  grantee: IamPrincipal | null;
}) {
  const command = useCommand(organization);
  const [kind, setKind] = React.useState<'user' | 'team'>('user');
  const [provider, setProvider] = React.useState(issuer ?? '');
  const [subject, setSubject] = React.useState('');
  const [team, setTeam] = React.useState('');
  const [role, setRole] = React.useState<IamProjectRole>('viewer');
  const [from, setFrom] = React.useState(() => localInputValue(new Date()));
  const [editUntil, setEditUntil] = React.useState('');
  const [readUntil, setReadUntil] = React.useState('');

  // The signed-in person's own issuer, once it arrives. Typing in the field
  // wins: an instance may hold principals from a provider it no longer uses.
  React.useEffect(() => {
    setProvider((held) => (held === '' && issuer ? issuer : held));
  }, [issuer]);

  // Picking somebody out of the roster fills both halves of the pair, because
  // a principal is compared exactly and a retyped 64-character subject is a
  // grant that silently matches nobody.
  React.useEffect(() => {
    if (!grantee) return;
    setKind('user');
    setProvider(grantee.provider);
    setSubject(grantee.subject);
  }, [grantee]);

  const issued =
    command.data && typeof command.data === 'object' && 'GrantCreated' in command.data
      ? command.data.GrantCreated
      : null;

  function submit(event: React.FormEvent) {
    event.preventDefault();
    const validFrom = unixFrom(from);
    if (validFrom === null) return;
    const grant: IamCommand = {
      type: 'grant',
      project,
      grantee:
        kind === 'user'
          ? { kind: 'user', value: { provider: provider.trim(), subject: subject.trim() } }
          : { kind: 'team', value: team.trim() },
      role,
      window: {
        valid_from: validFrom,
        edit_until: unixFrom(editUntil),
        read_until: unixFrom(readUntil),
      },
    };
    command.mutate(grant);
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Share this project</CardTitle>
      </CardHeader>
      <CardContent>
        <form className="flex flex-col gap-3 text-sm" onSubmit={submit}>
          <div className="flex flex-wrap gap-3">
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Grantee</span>
              <select
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={kind}
                onChange={(event) => setKind(event.target.value as 'user' | 'team')}
              >
                <option value="user">A person</option>
                <option value="team">A team</option>
              </select>
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Role</span>
              <select
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={role}
                onChange={(event) => setRole(event.target.value as IamProjectRole)}
              >
                {ROLES.map((entry) => (
                  <option key={entry} value={entry}>
                    {entry}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {kind === 'user' ? (
            <div className="flex flex-col gap-3 sm:flex-row">
              <label className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="text-xs text-muted-foreground">Provider</span>
                <input
                  className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
                  value={provider}
                  onChange={(event) => setProvider(event.target.value)}
                  placeholder="https://idp.example/application/o/aiwatcher/"
                />
              </label>
              <label className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="text-xs text-muted-foreground">Subject</span>
                <input
                  className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
                  value={subject}
                  onChange={(event) => setSubject(event.target.value)}
                  placeholder="the value their own Profile page shows"
                />
              </label>
            </div>
          ) : (
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">Team</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
                value={team}
                onChange={(event) => setTeam(event.target.value)}
                placeholder="the id the history shows when a team was created"
              />
            </label>
          )}

          <div className="flex flex-col gap-3 sm:flex-row">
            <label className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-xs text-muted-foreground">Access from</span>
              <input
                type="datetime-local"
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={from}
                onChange={(event) => setFrom(event.target.value)}
              />
            </label>
            <label className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-xs text-muted-foreground">Editing until</span>
              <input
                type="datetime-local"
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={editUntil}
                onChange={(event) => setEditUntil(event.target.value)}
              />
            </label>
            <label className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-xs text-muted-foreground">Reading until</span>
              <input
                type="datetime-local"
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={readUntil}
                onChange={(event) => setReadUntil(event.target.value)}
              />
            </label>
          </div>
          <p className="text-xs text-muted-foreground">
            An empty end is no end. Past <span className="font-medium">editing until</span> the
            grant becomes a viewer and keeps reading; past{' '}
            <span className="font-medium">reading until</span> it stops counting altogether. Another
            grant to the same person is a second source, and one expiring never hides the other.
          </p>

          <div className="flex items-center gap-3">
            <Button type="submit" disabled={command.isPending}>
              {command.isPending ? 'Granting…' : 'Grant'}
            </Button>
            {issued ? (
              <span className="text-xs text-muted-foreground">
                Granted. Its id is <span className="id">{issued.id}</span>, which is what revoking
                it needs.
              </span>
            ) : null}
          </div>
          {command.isError ? (
            <Refusal error={command.error} fallback="the grant was not issued" />
          ) : null}
        </form>
      </CardContent>
    </Card>
  );
}
