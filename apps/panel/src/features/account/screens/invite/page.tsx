import * as React from 'react';
import { useNavigate } from '@tanstack/react-router';

import { edgeOf, useEnrolment, useOffered, useRedeem } from '@/shared/lib/iam';
import { useAuthConfig, useSession, signIn } from '@/shared/lib/auth';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Refusal,
} from '@/shared/components/ui/primitives';

/** Where the token waits while the browser goes to the identity provider. */
const HELD = 'aiwatcher.invitation';

/**
 * The token a link carried, taken out of the URL before anything else runs.
 *
 * **The fragment, never the path or the query.** A fragment is not sent to a
 * server, so it is in no access log here and in none at the identity provider
 * the sign-in redirects through; a path segment would be in both. It is
 * removed from the address bar immediately and kept in `sessionStorage`,
 * because the round trip to the provider and back is a full page load and the
 * token has to survive it — one tab, closed when the tab is.
 *
 * This is a change to a rule this panel wrote down: *an invitation token is
 * pasted, never linked*. What that rule was protecting against was a secret in
 * a server log and in a URL somebody shares; the first is answered by the
 * fragment and the second is answered by the token being single-use and
 * short-lived. What it cost was the one case D5 exists for — somebody who has
 * no account cannot paste a token into a page they cannot reach.
 */
function heldToken(): string | null {
  if (typeof window === 'undefined') return null;
  const fragment = window.location.hash.replace(/^#/, '').trim();
  if (fragment) {
    try {
      window.sessionStorage.setItem(HELD, fragment);
    } catch {
      // A private window with storage blocked: the token still works for this
      // page load, which is enough for somebody who already has an account.
    }
    window.history.replaceState(null, '', window.location.pathname);
    return fragment;
  }
  try {
    return window.sessionStorage.getItem(HELD);
  } catch {
    return null;
  }
}

/**
 * What somebody sees when they follow an invitation link.
 *
 * Three states, and which one they are in is decided by what they already
 * have rather than by anything they choose: signed in here, signed in nowhere,
 * or holding a token this instance does not recognise. The offer's own terms
 * are shown in all three, because "create an account" is a decision and
 * nobody should take it without knowing what it is for.
 */
export function InvitePage() {
  const [token] = React.useState(heldToken);
  const offered = useOffered(token);
  const config = useAuthConfig();
  const session = useSession(config.data?.enabled === true);
  const redeem = useRedeem();
  const enrolment = useEnrolment();
  const navigate = useNavigate();

  if (!token) {
    return (
      <EmptyState
        title="This link carries no invitation"
        hint="Open the link you were sent, exactly as it was sent: what it offers is in the part after the #."
      />
    );
  }
  if (offered.isPending) return <EmptyState title="Reading the invitation…" />;
  if (offered.isError) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>This invitation cannot be taken up</CardTitle>
        </CardHeader>
        <CardContent className="text-sm">
          <Refusal
            error={offered.error}
            fallback="it may have been used already, or its offer may have lapsed"
          />
        </CardContent>
      </Card>
    );
  }

  const offer = offered.data;
  const signedIn = Boolean(session.data);
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>
            {offer.project.name}, in {offer.organization.name}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone="primary">{offer.role}</Badge>
            <span className="text-muted-foreground">
              from {edgeOf(offer.window.valid_from, 'always')}, editing until{' '}
              {edgeOf(offer.window.edit_until, 'no end')}, reading until{' '}
              {edgeOf(offer.window.read_until, 'no end')}
            </span>
          </div>
          <p className="text-xs text-muted-foreground">
            {offer.standing === 'guest'
              ? 'You would be a guest of this organization: this project and nothing else in it.'
              : 'You would be a member of this organization.'}{' '}
            The offer itself lapses {edgeOf(offer.expires_at, 'never')}.
          </p>

          {redeem.data ? (
            <p className="rounded-md border border-border p-3 text-xs">
              Done — {redeem.data.project.name} is yours to open.
            </p>
          ) : signedIn ? (
            <div className="flex flex-col gap-2">
              <Button
                onClick={() =>
                  redeem.mutate(token, {
                    onSuccess: (result) => {
                      try {
                        window.sessionStorage.removeItem(HELD);
                      } catch {
                        /* nothing to clear */
                      }
                      void navigate({
                        to: '/observability/runs',
                        search: {
                          scope: `${result.organization.id}/${result.project.scope.project}`,
                        },
                      });
                    },
                  })
                }
                disabled={redeem.isPending}
              >
                {redeem.isPending ? 'Accepting…' : 'Accept'}
              </Button>
              {redeem.isError ? (
                <Refusal error={redeem.error} fallback="the token was not accepted" />
              ) : null}
            </div>
          ) : (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                onClick={() =>
                  enrolment.mutate(token, {
                    onSuccess: (where) => window.location.assign(where.url),
                  })
                }
                disabled={enrolment.isPending}
              >
                {enrolment.isPending ? 'Opening…' : 'Create an account'}
              </Button>
              <Button variant="outline" onClick={() => signIn()}>
                I already have one
              </Button>
              {enrolment.isError ? (
                <Refusal
                  error={enrolment.error}
                  fallback="this instance cannot open an account for you; sign in with one you already have"
                />
              ) : null}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
