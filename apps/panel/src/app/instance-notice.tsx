import { useRouterState } from '@tanstack/react-router';
import { Building2 } from 'lucide-react';

import { reachOf } from '@/app/navigation';
import { useAuthConfig, useSession } from '@/shared/lib/auth';
import { useGrantedProjects } from '@/shared/lib/iam';
import { Card, CardContent } from '@/shared/components/ui/primitives';

/**
 * Whether this caller holds a role on the deployment itself.
 *
 * `false` is a real, signed-in person who holds nothing here outside the
 * projects they were granted — what `AIWATCHER_AUTH_DEFAULT_ROLE=project`
 * produces, and what every client on a shared instance is. `undefined` while
 * the session is still being read, because "you may not" is a sentence nobody
 * has issued yet.
 */
export function useInstanceRole(): boolean | undefined {
  const config = useAuthConfig();
  const enabled = config.data?.enabled === true;
  const session = useSession(enabled);
  if (!config.isSuccess) return undefined;
  // Nobody to refuse: with authentication off every role check passes, and a
  // panel that hid its own pages on a single-user install would be inventing a
  // permission system nobody configured.
  if (!enabled) return true;
  if (!session.isFetched) return undefined;
  return (session.data?.roles.length ?? 0) > 0;
}

/**
 * What an instance area shows somebody who holds no role on the deployment.
 *
 * In place of the page, not beside it. Every read on an instance area answers
 * that caller 403, so without this the screen is a row of failed queries —
 * which reads as *aiwatcher is broken* rather than as *this part is the
 * deployment's own*. The distinction is the whole of IAM-03's D4 from the
 * reader's side: they are signed in, nothing is wrong, and what is on this
 * page is not theirs.
 *
 * It says where their work is rather than only what this is not, because
 * somebody who has just been told "not yours" needs the next click more than
 * they need the rule.
 */
export function InstanceOnly({ children }: { children: React.ReactNode }) {
  const holdsRole = useInstanceRole();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const { projects } = useGrantedProjects();
  const reach = reachOf(pathname);

  // `undefined` renders the page: a blank screen while the session loads is
  // worse than a page that settles, and the server refuses either way.
  if (holdsRole !== false) return children;
  if (reach !== 'instance' && reach !== 'mixed') return children;

  // Named, because "your work is elsewhere" is the useful half and the names
  // are what somebody clicks next.
  const names = projects.map((entry) => entry.access.project.name);
  return (
    <Card>
      <CardContent className="flex items-start gap-3 py-6">
        <Building2 className="mt-0.5 h-5 w-5 shrink-0 text-muted-foreground" />
        <div className="space-y-2 text-sm">
          <p className="font-medium">This part belongs to the deployment, not to you.</p>
          <p className="text-muted-foreground">
            {reach === 'instance'
              ? 'It answers for the whole instance — every project’s work and the work that belongs to none — so it is read by whoever administers the instance rather than by whoever holds a project.'
              : 'Some of it answers for the whole instance — every project’s work and the work that belongs to none — so it is read by whoever administers the instance rather than by whoever holds a project.'}
          </p>
          <p className="text-muted-foreground">
            {names.length === 0
              ? 'You hold no project here yet. An invitation is what puts one on this list.'
              : `Your own runs, prompts and evaluations are in ${names.join(', ')} — pick it in the header and the areas it reaches will answer.`}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}
