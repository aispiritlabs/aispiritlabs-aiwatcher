import * as React from 'react';
import { LogOut } from 'lucide-react';

import type { Identity } from '@/api/generated';
import { displayNameOf, initialsOf, signOut, useAuthConfig, useSession } from '@/lib/auth';
import { Badge, Button } from '@/components/ui/primitives';

/**
 * Who is signed in, in the header.
 *
 * Renders nothing on an instance with no provider — a chip saying "anonymous"
 * would be a control that implies a choice nobody has.
 *
 * The role is shown beside the name rather than hidden behind a click, because
 * it is the answer to the question people actually arrive with: why is the
 * rerun button refusing me. The fix is a group in the identity provider, and
 * knowing which role you hold is the first half of asking for it.
 */
export function UserMenu() {
  const config = useAuthConfig();
  const enabled = config.data?.enabled === true;
  const session = useSession(enabled);
  const [open, setOpen] = React.useState(false);

  if (!enabled || !session.data) return null;
  const identity = session.data;
  // `proxy` mode declares no logout URL, and that is the honest answer: the
  // session belongs to the proxy in front, so a button here would clear
  // nothing and read as a sign-out that did not work.
  const maySignOut = config.data?.logout_url != null;

  return (
    <div className="relative ml-auto shrink-0">
      <button
        type="button"
        onClick={() => setOpen((was) => !was)}
        className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        title={displayNameOf(identity)}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span className="flex h-6 w-6 items-center justify-center rounded-full bg-primary/15 text-[10px] font-semibold text-primary">
          {initialsOf(identity)}
        </span>
        {/* Initials alone until there is room for more. The header carries six
            areas beside this, and a name that pushes them off the screen is a
            worse trade than a name you hover to read. */}
        <span className="hidden xl:inline">{displayNameOf(identity)}</span>
        <RoleBadge identity={identity} />
      </button>

      {open && (
        <>
          {/* Closes on any click outside, which is what a menu without Radix
              has to do for itself. Radix goes in with the first dialog. */}
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div
            role="menu"
            className="absolute right-0 z-20 mt-1 w-64 rounded-md border border-border bg-card p-3 shadow-lg"
          >
            <div className="flex flex-col gap-0.5 pb-2">
              <span className="text-sm font-medium">{displayNameOf(identity)}</span>
              {identity.email && (
                <span className="text-xs text-muted-foreground">{identity.email}</span>
              )}
            </div>

            {identity.groups && identity.groups.length > 0 && (
              <div className="flex flex-col gap-1 border-t border-border py-2">
                <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                  Groups
                </span>
                <div className="flex flex-wrap gap-1">
                  {identity.groups.map((group) => (
                    <Badge key={group} tone="neutral">
                      {group}
                    </Badge>
                  ))}
                </div>
              </div>
            )}

            {maySignOut ? (
              <Button
                variant="ghost"
                size="sm"
                className="mt-1 w-full justify-start"
                onClick={() => void signOut()}
              >
                <LogOut className="mr-2 h-3.5 w-3.5" />
                Sign out
              </Button>
            ) : (
              <p className="border-t border-border pt-2 text-xs text-muted-foreground">
                Signed in through {config.data?.provider ?? 'the proxy'} in front of aiwatcher. Sign
                out there.
              </p>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function RoleBadge({ identity }: { identity: Identity }) {
  const highest = identity.roles.at(-1) ?? 'viewer';
  return (
    <Badge tone={highest === 'admin' ? 'primary' : 'neutral'} className="hidden xl:inline-flex">
      {highest}
    </Badge>
  );
}
