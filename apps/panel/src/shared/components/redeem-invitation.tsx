import * as React from 'react';

import { edgeOf, useRedeem } from '@/shared/lib/iam';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Refusal,
} from '@/shared/components/ui/primitives';

/**
 * The other end of an invitation: somebody pastes what they were sent.
 *
 * A field rather than a link. An invitation token in a URL is a secret in
 * browser history, in a referrer header and in whatever chat it was pasted
 * into — and the thing it buys, one click instead of one paste, is not worth
 * that. It also keeps the token out of this panel's URL contract, where every
 * other parameter is something a person may safely share.
 *
 * Redeeming is a write by somebody who is not yet a member, which is the whole
 * point: the server learns their `(provider, subject)` pair from the session
 * they are already holding, and nothing they typed names them.
 *
 * Both areas that deal in grants draw it, and the framing is the only thing
 * that differs: an administrator was invited to a project, a participant was
 * enrolled in a workshop, and the token is the same token.
 */
export function Redeem({
  title,
  intro,
  onRedeemed,
}: {
  title: string;
  /** What arriving here with a token means, in the reader's own terms. */
  intro: React.ReactNode;
  onRedeemed: (organization: string, project: string) => void;
}) {
  const redeem = useRedeem();
  const [token, setToken] = React.useState('');

  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        <p className="text-xs text-muted-foreground">{intro}</p>
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (!token.trim()) return;
            redeem.mutate(token.trim(), {
              onSuccess: (result) => {
                setToken('');
                onRedeemed(result.organization.id, result.project.scope.project);
              },
            });
          }}
        >
          <label className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="text-xs text-muted-foreground">Token</span>
            <input
              className="h-9 rounded-md border border-border bg-transparent px-2 font-mono text-xs"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <Button type="submit" disabled={redeem.isPending || !token.trim()}>
            {redeem.isPending ? 'Redeeming…' : 'Redeem'}
          </Button>
        </form>
        {redeem.isError ? (
          <Refusal error={redeem.error} fallback="the token was not accepted" />
        ) : null}
        {redeem.data ? (
          <div className="flex flex-wrap items-center gap-2 rounded-md border border-border p-3 text-xs">
            <Badge tone="success">{redeem.data.role}</Badge>
            <span>
              on {redeem.data.project.name} in {redeem.data.organization.name}, from{' '}
              {edgeOf(redeem.data.window.valid_from, 'always')}, editing until{' '}
              {edgeOf(redeem.data.window.edit_until, 'no end')}, reading until{' '}
              {edgeOf(redeem.data.window.read_until, 'no end')}
            </span>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}
