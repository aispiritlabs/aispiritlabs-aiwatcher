import {
  PHASES,
  participantsOf,
  phaseOf,
  windowSentence,
} from '@/features/learning/lib/enrollment';
import { edgeOf, short, useProjectGrants } from '@/shared/lib/iam';
import {
  Badge,
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

/**
 * Who is enrolled, and where each grant sits in its window.
 *
 * Every grant on the project, as issued and in full — a lapsed one is usually
 * what somebody opened this list to find, and it is also the evidence that a
 * lesson ended rather than that somebody was removed. Grants are grouped by
 * the person they name, because two of them on one person is the ordinary case
 * this design turns on: a permanent grant and a workshop one, whose expiry must
 * not read as access being gone.
 *
 * The phase on each row is read from that row's own dates, and `enrollment.ts`
 * says why that is as far as it goes. Nothing here is summed into "what this
 * person may do now": the effective role is a maximum over live grants, the
 * server takes it per request, and it answers only about the caller.
 */
export function Participants({
  organization,
  project,
  now,
}: {
  organization: string;
  project: string;
  /** Unix seconds, passed in so a test can put the clock inside a window. */
  now: number;
}) {
  const grants = useProjectGrants(organization, project);
  const forbidden = grants.error instanceof ApiFailure && grants.error.status === 403;
  const participants = participantsOf(grants.data ?? []);

  return (
    <Card>
      <CardHeader>
        <CardTitle>Participants</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        <p className="text-xs text-muted-foreground">
          A participant is a grant on this project. Each row says where that one grant sits in the
          window it was issued with — this panel reads the dates, it does not take the decision: the
          server does that per request, and it answers about you alone. Somebody whose workshop
          grant has closed may still hold another that has not.
        </p>
        {grants.isPending ? <Spinner /> : null}
        {forbidden ? (
          <p className="text-xs text-muted-foreground">
            The list of participants is for whoever may enrol one — this project&rsquo;s admin, or
            an admin of the organization.
          </p>
        ) : grants.isError ? (
          <Refusal error={grants.error} fallback="the participants could not be read" />
        ) : null}
        {grants.data?.length === 0 ? (
          <EmptyState
            title="Nobody is enrolled yet"
            hint="An invitation below is how somebody joins who has never signed in here."
          />
        ) : null}
        <ul className="flex flex-col gap-2">
          {participants.map((participant) => (
            <li
              key={participant.key}
              className="flex flex-col gap-2 rounded-md border border-border p-3"
            >
              <div className="flex flex-wrap items-center gap-2 text-xs">
                {participant.grantee.kind === 'user' ? (
                  <>
                    <IdChip
                      label="subject"
                      value={short(participant.grantee.value.subject, 16)}
                      full={participant.grantee.value.subject}
                    />
                    <span className="break-all text-muted-foreground">
                      at {participant.grantee.value.provider}
                    </span>
                  </>
                ) : (
                  <span className="flex items-center gap-1">
                    everyone on the team{' '}
                    <IdChip
                      label="team"
                      value={short(participant.grantee.value)}
                      full={participant.grantee.value}
                    />
                  </span>
                )}
                {participant.grants.length > 1 ? (
                  <span className="text-muted-foreground">
                    {participant.grants.length} grants, each in force on its own
                  </span>
                ) : null}
              </div>
              <ul className="flex flex-col gap-1">
                {participant.grants.map((grant) => {
                  const phase = phaseOf(grant.window, grant.role, now);
                  return (
                    <li key={grant.id} className="flex flex-wrap items-center gap-2 text-xs">
                      <Badge tone={PHASES[phase].tone}>{PHASES[phase].label}</Badge>
                      <Badge>{grant.role}</Badge>
                      <span className="text-muted-foreground">
                        {windowSentence(grant.window, phase)}
                      </span>
                      <span className="text-muted-foreground">
                        from {edgeOf(grant.window.valid_from, 'always')}
                      </span>
                      <IdChip label="grant" value={short(grant.id)} full={grant.id} />
                    </li>
                  );
                })}
              </ul>
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}
