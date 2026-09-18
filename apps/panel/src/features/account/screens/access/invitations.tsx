import * as React from 'react';

import type { IamProjectRole } from '@/api/generated';
import {
  edgeOf,
  localInputValue,
  short,
  unixFrom,
  useInvitations,
  useInvite,
  useRevokeInvitation,
} from '@/features/account/iam';
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

const ROLES: IamProjectRole[] = ['viewer', 'editor', 'admin'];
const WEEK_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * Offering a project to somebody who has never signed in here.
 *
 * A grant names a `(provider, subject)` pair, and nobody knows a stranger's
 * subject until their provider has minted one — so until now the only way to
 * share a lesson was to ask the person to sign in first and read their subject
 * out. An invitation is the offer made to a secret instead: whoever presents
 * the token becomes a member and gets the declared grant, once, and the pair is
 * learned from their verified session at that moment.
 *
 * The token appears exactly once, here, in the response that created it. The
 * record kept afterwards holds only a digest, so this card can say what was
 * offered and whether it was taken, and can never show the secret again.
 */
export function Invitations({
  organization,
  project,
  projectName,
}: {
  organization: string;
  project: string;
  projectName: string;
}) {
  const offers = useInvitations(organization);
  const invite = useInvite(organization, project);
  const withdraw = useRevokeInvitation(organization);
  const [role, setRole] = React.useState<IamProjectRole>('viewer');
  const [label, setLabel] = React.useState('');
  const [from, setFrom] = React.useState(() => localInputValue(new Date()));
  const [editUntil, setEditUntil] = React.useState('');
  const [readUntil, setReadUntil] = React.useState('');
  const [expires, setExpires] = React.useState(() =>
    localInputValue(new Date(Date.now() + WEEK_MS)),
  );

  const mine = offers.data?.filter((offer) => offer.scope.project === project) ?? [];
  const forbidden = offers.error instanceof ApiFailure && offers.error.status === 403;

  function submit(event: React.FormEvent) {
    event.preventDefault();
    const validFrom = unixFrom(from);
    const expiresAt = unixFrom(expires);
    if (validFrom === null || expiresAt === null) return;
    invite.mutate({
      role,
      window: {
        valid_from: validFrom,
        edit_until: unixFrom(editUntil),
        read_until: unixFrom(readUntil),
      },
      expires_at: expiresAt,
      label: label.trim() || undefined,
    });
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Invite somebody to {projectName}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 text-sm">
        <p className="text-xs text-muted-foreground">
          For somebody who has not signed in here yet, so there is no subject to grant to. They
          redeem the token below on their own Organizations &amp; projects page, and become a member
          of this organization with exactly the grant declared here.
        </p>

        <form className="flex flex-col gap-3" onSubmit={submit}>
          <div className="flex flex-wrap gap-3">
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
            <label className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-xs text-muted-foreground">Who it is for, as a note</span>
              <input
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={label}
                onChange={(event) => setLabel(event.target.value)}
                placeholder="somebody@example.com"
              />
            </label>
          </div>

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
            <label className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-xs text-muted-foreground">The offer lapses</span>
              <input
                type="datetime-local"
                className="h-9 rounded-md border border-border bg-transparent px-2"
                value={expires}
                onChange={(event) => setExpires(event.target.value)}
              />
            </label>
          </div>
          <p className="text-xs text-muted-foreground">
            The offer lapsing and the access ending are two different clocks: the first says how
            long somebody has to accept, the second is the window they get when they do.
          </p>
          <div>
            <Button type="submit" disabled={invite.isPending}>
              {invite.isPending ? 'Creating…' : 'Create invitation'}
            </Button>
          </div>
          {invite.isError ? (
            <Refusal error={invite.error} fallback="the invitation was not created" />
          ) : null}
        </form>

        {invite.data ? (
          <div className="flex flex-col gap-1 rounded-md border border-primary/40 bg-primary/5 p-3">
            <p className="text-xs font-medium">
              Send this to them. It is shown once and nothing can show it again.
            </p>
            <IdChip label="token" value={invite.data.token} />
          </div>
        ) : null}

        <div className="flex flex-col gap-2 border-t border-border pt-4">
          <h3 className="text-xs font-medium text-muted-foreground">Open and spent offers</h3>
          {offers.isPending ? <Spinner /> : null}
          {forbidden ? (
            <p className="text-xs text-muted-foreground">
              Invitations are listed for whoever may issue one here.
            </p>
          ) : offers.isError ? (
            <Refusal error={offers.error} fallback="the invitations could not be read" />
          ) : null}
          {offers.data && mine.length === 0 ? (
            <EmptyState title="Nobody has been invited to this project" />
          ) : null}
          <ul className="flex flex-col gap-2">
            {mine.map((offer) => (
              <li
                key={offer.id}
                className="flex flex-col gap-1 rounded-md border border-border p-3 text-xs"
              >
                <div className="flex flex-wrap items-center gap-2">
                  <Badge tone={offer.redeemed ? 'success' : 'neutral'}>
                    {offer.redeemed ? 'redeemed' : 'open'}
                  </Badge>
                  <Badge>{offer.role}</Badge>
                  {offer.label ? <span>{offer.label}</span> : null}
                  {offer.redeemed ? null : (
                    <Button
                      size="sm"
                      variant="outline"
                      className="ml-auto"
                      disabled={withdraw.isPending}
                      onClick={() => withdraw.mutate(offer.id)}
                    >
                      Withdraw
                    </Button>
                  )}
                </div>
                <div className="text-muted-foreground">
                  {offer.redeemed ? (
                    <>
                      taken {new Date(offer.redeemed.at * 1000).toLocaleString()} by{' '}
                      <IdChip
                        label="subject"
                        value={short(offer.redeemed.principal.subject)}
                        full={offer.redeemed.principal.subject}
                      />
                    </>
                  ) : (
                    <>lapses {edgeOf(offer.expires_at, 'never')}</>
                  )}
                </div>
                <div className="text-muted-foreground">
                  grants from {edgeOf(offer.window.valid_from, 'always')}, editing until{' '}
                  {edgeOf(offer.window.edit_until, 'no end')}, reading until{' '}
                  {edgeOf(offer.window.read_until, 'no end')}
                </div>
              </li>
            ))}
          </ul>
          {withdraw.isError ? (
            <Refusal error={withdraw.error} fallback="the invitation was not withdrawn" />
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}
