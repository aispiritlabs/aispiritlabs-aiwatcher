import { displayNameOf, useAuthConfig, useSession } from '@/shared/lib/auth';
import { Card, CardContent, CardHeader, CardTitle, Badge } from '@/shared/components/ui/primitives';

export function ProfilePage() {
  const config = useAuthConfig();
  const session = useSession(config.data?.enabled === true);
  const identity = session.data;
  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <h1 className="text-lg font-semibold">Profile & account</h1>
      {!config.data?.enabled ? <p>Local instance: sign-in is disabled. There is no signed-in account.</p> : identity ? <>
        <Card>
          <CardHeader><CardTitle>{displayNameOf(identity)}</CardTitle></CardHeader>
          <CardContent className="space-y-3 text-sm">
            <dl className="grid gap-2 sm:grid-cols-[10rem_1fr]">
              <dt className="text-muted-foreground">Email</dt><dd>{identity.email ?? 'Not provided'}</dd>
              <dt className="text-muted-foreground">Identity provider</dt><dd>{config.data.provider ?? config.data.mode}</dd>
              <dt className="text-muted-foreground">Subject</dt><dd className="break-all font-mono text-xs">{identity.subject}</dd>
              <dt className="text-muted-foreground">Instance roles</dt><dd>{identity.roles.join(', ') || 'None'}</dd>
            </dl>
            <p className="text-muted-foreground">Your identity, password and account recovery are managed by your identity provider.</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle>Identity provider groups</CardTitle></CardHeader>
          <CardContent className="space-y-3 text-sm">
            <div className="flex flex-wrap gap-2">{(identity.groups ?? []).map((group) => <Badge key={group}>{group}</Badge>)}</div>
            {(identity.groups ?? []).length === 0 && <p>No groups were provided for this session.</p>}
            <p className="text-muted-foreground">Read-only SSO groups, and the whole of your instance role. They are never project teams: membership, teams and timed access are aiwatcher&rsquo;s own, under Organizations &amp; projects.</p>
          </CardContent>
        </Card>
      </> : <p role="status">Loading account…</p>}
    </div>
  );
}
