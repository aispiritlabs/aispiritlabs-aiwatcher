import * as React from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { Activity, LogIn, ShieldAlert } from 'lucide-react';

import { client } from '@/lib/api';
import {
  SESSION_KEY,
  signIn,
  signInError,
  signInErrorMessage,
  useAuthConfig,
  useSession,
} from '@/lib/auth';
import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Spinner,
} from '@/components/ui/primitives';

/**
 * Everything the panel renders sits behind this.
 *
 * Above the router rather than inside it, for two reasons. A route that
 * required a session would have to be told so one route at a time, and the one
 * forgotten is the one that leaks. And the sign-in screen is not a page: there
 * is no navigation to it and no URL for it — it is what the whole application
 * looks like when nobody is signed in.
 *
 * On an instance with no provider this renders its children and nothing else
 * happens, which is what `AIWATCHER_AUTH_MODE=none` has to look like.
 */
export function AuthGate({ children }: { children: React.ReactNode }) {
  const config = useAuthConfig();
  const enabled = config.data?.enabled === true;
  const session = useSession(enabled);
  const queryClient = useQueryClient();

  // A 401 from any route means the session ended while the tab was open —
  // expired, or signed out in another tab. Asking again here is what turns
  // that into the sign-in screen instead of a page of failed requests.
  //
  // Every route except the one that answers the question. `/auth/me` replies
  // 401 to mean "nobody is signed in", which is its answer rather than a
  // failure; invalidating on it would refetch it, get the same 401, and
  // invalidate again for as long as the tab stayed open.
  React.useEffect(() => {
    if (!enabled) return;
    const interceptor = client.interceptors.response.use((response: Response) => {
      if (response.status === 401 && !new URL(response.url).pathname.startsWith('/api/v1/auth/')) {
        void queryClient.invalidateQueries({ queryKey: SESSION_KEY });
      }
      return response;
    });
    return () => {
      client.interceptors.response.eject(interceptor);
    };
  }, [enabled, queryClient]);

  // Nothing is known yet. A flash of the sign-in screen on every load would be
  // worse than a moment of nothing, because it reads as having been signed out.
  if (config.isPending || (enabled && session.isPending)) {
    return (
      <div className="flex min-h-screen items-center justify-center">
        <Spinner />
      </div>
    );
  }

  if (!enabled) return <>{children}</>;

  if (session.isError) {
    return (
      <SignInScreen
        provider={config.data?.provider ?? 'the identity provider'}
        mode={config.data?.mode ?? 'oidc'}
        title="The identity provider could not be reached"
        detail="Signing in again will not help until it answers. The server's log says what it tried."
      />
    );
  }

  if (!session.data) {
    return (
      <SignInScreen
        provider={config.data?.provider ?? 'the identity provider'}
        mode={config.data?.mode ?? 'oidc'}
        detail={signInError ? signInErrorMessage(signInError) : undefined}
      />
    );
  }

  return <>{children}</>;
}

function SignInScreen({
  provider,
  mode,
  title,
  detail,
}: {
  provider: string;
  mode: string;
  title?: string;
  detail?: string;
}) {
  return (
    <div className="flex min-h-screen items-center justify-center px-6">
      <Card className="w-full max-w-md">
        <CardHeader className="flex flex-col gap-1">
          <div className="flex items-center gap-2 font-semibold">
            <Activity className="h-4 w-4 text-primary" />
            aiwatcher
          </div>
          <CardTitle className="text-sm font-normal text-muted-foreground">
            {title ?? 'Sign in to continue'}
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {detail && (
            <p className="flex items-start gap-2 rounded-md border border-border bg-muted/40 p-3 text-xs leading-relaxed text-muted-foreground">
              <ShieldAlert className="mt-0.5 h-3.5 w-3.5 shrink-0 text-warning" />
              <span>{detail}</span>
            </p>
          )}

          {mode === 'proxy' ? (
            // There is no button to offer: in this mode the sign-in happened
            // at the proxy before the request reached aiwatcher, so seeing
            // this screen means the proxy let a request through without
            // setting its identity headers.
            <p className="text-xs leading-relaxed text-muted-foreground">
              This instance takes its identity from the proxy in front of it, and this request
              arrived without it. Reload the page; if it keeps happening, the proxy is not
              authenticating this route.
            </p>
          ) : (
            <Button onClick={signIn} className="w-full">
              <LogIn className="mr-2 h-3.5 w-3.5" />
              Sign in with {provider}
            </Button>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
