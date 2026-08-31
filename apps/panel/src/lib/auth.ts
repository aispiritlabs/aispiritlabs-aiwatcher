import { useQuery, type UseQueryResult } from '@tanstack/react-query';

import { authConfig, logout, me } from '@/api/generated';
import type { Identity, PublicAuthConfig, Role } from '@/api/generated';

/**
 * What the panel knows about who is using it.
 *
 * Two questions, asked separately because they have different answers and
 * different lifetimes. *Is there a login on this instance* is a deployment
 * fact that never changes while the tab is open, and it has to be answerable
 * before anybody has signed in — otherwise the panel either shows a sign-in
 * screen on an instance with no provider or loops on 401 against one that has.
 * *Who is signed in* changes, and a 401 is its answer rather than an error.
 */

const CONFIG_KEY = ['auth', 'config'] as const;
export const SESSION_KEY = ['auth', 'session'] as const;

/**
 * Why the last sign-in did not happen, if it did not.
 *
 * Read at module load and removed from the address bar immediately. The server
 * puts it there because the callback is a top-level navigation — whatever it
 * returns is what the person sees — and it has to be captured before the
 * router gets a chance to redirect `/` somewhere else and drop the query
 * string with it.
 */
export const signInError: string | null = (() => {
  if (typeof window === 'undefined') return null;
  const url = new URL(window.location.href);
  const error = url.searchParams.get('sign_in_error');
  if (error) {
    url.searchParams.delete('sign_in_error');
    window.history.replaceState({}, '', url.toString());
  }
  return error;
})();

export function useAuthConfig(): UseQueryResult<PublicAuthConfig> {
  return useQuery({
    queryKey: CONFIG_KEY,
    queryFn: async () => {
      const { data, error } = await authConfig();
      if (!data) throw error ?? new Error('the instance did not answer /auth/config');
      return data;
    },
    // A deployment fact. Refetching it would only ever return the same answer.
    staleTime: Infinity,
    retry: 1,
  });
}

/**
 * The current caller, or `null` when nobody is signed in.
 *
 * `null` rather than an error: a 401 here is not a failure, it is the answer.
 * Anything else — the provider being unreachable, say — still throws, because
 * "we cannot tell" and "nobody is signed in" call for different screens.
 */
export function useSession(enabled: boolean): UseQueryResult<Identity | null> {
  return useQuery({
    queryKey: SESSION_KEY,
    enabled,
    queryFn: async () => {
      const { data, response } = await me();
      if (response?.status === 401) return null;
      if (!data) throw new Error(`/auth/me answered ${response?.status ?? 'nothing'}`);
      return data;
    },
    retry: false,
    staleTime: 60_000,
    refetchOnWindowFocus: true,
  });
}

/**
 * Leave for the identity provider, and come back here.
 *
 * A full navigation rather than a fetch: the whole point of the flow is that
 * the browser visits the provider, and an XHR could neither show its login
 * form nor carry its cookies.
 */
export function signIn(): void {
  const next = window.location.pathname + window.location.search;
  window.location.assign(`/api/v1/auth/login?next=${encodeURIComponent(next)}`);
}

/**
 * Sign out here, and then at the provider.
 *
 * Both halves, because clearing only this session would put the user back in
 * with one click — which reads as a sign-out that did not work.
 */
export async function signOut(): Promise<void> {
  const { data } = await logout();
  window.location.assign(data?.redirect_url ?? '/');
}

const RANK: Record<Role, number> = { viewer: 0, editor: 1, admin: 2 };

/** Whether the caller holds at least `needed`. */
export function can(identity: Identity | null | undefined, needed: Role): boolean {
  if (!identity) return false;
  const held = identity.roles.reduce((highest, role) => Math.max(highest, RANK[role]), -1);
  return held >= RANK[needed];
}

/**
 * Whether the current caller may do something needing `needed`.
 *
 * `true` on an instance with no provider, which is what makes this safe to put
 * on every write control: with authentication off there is nobody to refuse,
 * and a control greyed out on a single-user deployment would be a permission
 * system nobody asked for.
 *
 * This hides a button the server would refuse anyway. The server is the check;
 * this is so that finding out does not cost a round trip and a red toast.
 */
export function useCan(needed: Role): boolean {
  const config = useAuthConfig();
  const enabled = config.data?.enabled === true;
  const session = useSession(enabled);
  return enabled ? can(session.data, needed) : true;
}

/** What to tell somebody about a control they may not use. */
export function needsRole(needed: Role): string {
  return `This needs the ${needed} role. Ask an administrator to add you to the matching group in the identity provider.`;
}

/** Initials for the header chip: two letters, from whatever the provider sent. */
export function initialsOf(identity: Identity): string {
  const source = identity.name ?? identity.username ?? identity.email ?? identity.subject;
  const [first, second] = source.split(/[\s._@-]+/).filter(Boolean);
  const letters = first && second ? first.charAt(0) + second.charAt(0) : source.slice(0, 2);
  return letters.toUpperCase();
}

/** What to show as the caller's name, in the order a person would recognise. */
export function displayNameOf(identity: Identity): string {
  return identity.name ?? identity.username ?? identity.email ?? identity.subject;
}

/**
 * The message for a sign-in that did not complete.
 *
 * Named cases only for the ones an operator can act on. Everything else is one
 * sentence: the detail is in the server's log, where it belongs, because the
 * difference between a bad signature and a wrong audience is useful to whoever
 * is attacking and to nobody else.
 */
export function signInErrorMessage(code: string): string {
  switch (code) {
    case 'access_denied':
      return 'The identity provider refused the sign-in.';
    case 'not_entitled':
      return 'You signed in, but this instance grants you no access. Ask an administrator to add you to one of its groups.';
    case 'state_mismatch':
      return 'That sign-in took too long or was started somewhere else. Try again.';
    case 'exchange_failed':
      return 'The identity provider rejected this application. Its client id, secret or redirect URI is wrong — the server log says which.';
    case 'not_configured':
      return 'This instance has no identity provider configured.';
    default:
      return 'The sign-in could not be completed.';
  }
}
