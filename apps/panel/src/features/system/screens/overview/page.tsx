import { useQuery } from '@tanstack/react-query';

import { system } from '@/api/generated';
import type {
  SystemCapability,
  SystemCapabilityGroup,
  SystemInventory,
} from '@/api/generated';
import {
  Badge,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Refusal,
  Spinner,
} from '@/shared/components/ui/primitives';
import { ApiFailure, answerOf } from '@/shared/lib/result';

/**
 * What this instance is: every capability, its state, and the variable that
 * decides it.
 *
 * The one area here that describes the **deployment** rather than anything it
 * holds. Everything else on this panel reads runs, or an authored artifact, or
 * a control plane; this reads configuration, and it exists because until now
 * the answer to "does this instance have a prompt store" was reachable only by
 * opening Prompts and being told 501. Those refusals are still right where they
 * are — they arrive where somebody is blocked — and this is the same set of
 * facts read out once, before anybody is.
 *
 * Three rules hold it, and all three are the server's.
 *
 * **Nothing here is computed in the browser.** A state, the variable that
 * decides it, the note under it and every value are fields of `GET
 * /api/v1/system`. A panel that worked out "no store, so Prompts is off" would
 * be a second copy of `AppState`'s conditions in another language, and the day
 * they disagreed this screen would be the confident one.
 *
 * **A value that is missing is missing on purpose.** That a thing is configured
 * is not a secret and its value often is, so the API withholds every
 * credential and every address — a database URL, an object store's endpoint, a
 * judge's host — and sends the *variable name* instead. A row with a variable
 * and no value is this working, not a gap: the reader gets what to set without
 * being handed reconnaissance. The issuer is the single exception, and it is
 * the server's exception rather than this screen's.
 *
 * **It is read-only, and that is the design.** Changing a setting is an
 * environment variable and a restart. A form here would promise a change that
 * nothing durable explains, on a process that would forget it on the next
 * rollout.
 *
 * It needs the `admin` role, which is the only refusal drawn as a sentence
 * rather than as a failure: an inventory of the deployment is an operator's
 * question, and somebody reading a run being told no is an answer rather than
 * something going wrong.
 */

const GROUPS: Array<{ id: SystemCapabilityGroup; label: string; blurb: string }> = [
  {
    id: 'identity',
    label: 'Identity',
    blurb: 'Who the caller is, and who decides what they may reach.',
  },
  {
    id: 'observability',
    label: 'Observability',
    blurb: 'The log, what folds it, and what it is read against.',
  },
  {
    id: 'storage',
    label: 'Storage',
    blurb: 'Where the things that outlive a run are kept.',
  },
  {
    id: 'runtime',
    label: 'Runtimes',
    blurb: 'What work runs here, and under what.',
  },
  {
    id: 'integration',
    label: 'Integrations',
    blurb: 'What leaves the building: a service this deployment reaches but does not run.',
  },
];

function useSystem() {
  return useQuery<SystemInventory>({
    queryKey: ['system'],
    queryFn: async () =>
      answerOf(await system(), 'this instance did not answer what it is configured with'),
    // Configuration changes with a restart, so re-asking on every focus would
    // be a request per tab switch for an answer that cannot have moved. It is
    // still not `Infinity`: a rolling upgrade does change it, and a stale
    // inventory is the one thing this screen must not be.
    staleTime: 60_000,
    retry: false,
  });
}

export function SystemPage() {
  const inventory = useSystem();

  return (
    <div className="flex max-w-5xl flex-col gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">System</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            What this instance has configured, and the environment variable that decides each. Read
            only: a setting is changed where the process is started, not here.
          </p>
        </div>
        {inventory.data ? (
          <Badge tone="neutral" className="font-mono">
            v{inventory.data.version}
          </Badge>
        ) : null}
      </div>

      {inventory.isPending ? (
        <p className="flex items-center gap-2 text-sm text-muted-foreground">
          <Spinner /> Reading this instance’s configuration…
        </p>
      ) : null}

      {inventory.error ? <Unavailable error={inventory.error} /> : null}

      {inventory.data
        ? GROUPS.map((group) => {
            const rows = inventory.data.capabilities.filter(
              (capability) => capability.group === group.id,
            );
            if (rows.length === 0) return null;
            return (
              <Card key={group.id}>
                <CardHeader>
                  <CardTitle>{group.label}</CardTitle>
                  <p className="text-xs text-muted-foreground">{group.blurb}</p>
                </CardHeader>
                <CardContent className="flex flex-col divide-y divide-border">
                  {rows.map((capability) => (
                    <CapabilityRow key={capability.id} capability={capability} />
                  ))}
                </CardContent>
              </Card>
            );
          })
        : null}
    </div>
  );
}

/**
 * A refused read, in the two shapes it comes in.
 *
 * A 403 here is the server answering rather than failing, so it is a sentence.
 * Anything else is a failure and is drawn as one — an inventory that rendered a
 * broken instance as "nothing is configured" would be the worst possible lie
 * for this particular screen.
 */
function Unavailable({ error }: { error: unknown }) {
  if (error instanceof ApiFailure && error.status === 403) {
    return (
      <Card className="border-dashed">
        <CardHeader>
          <CardTitle className="text-sm">Reading this instance’s configuration needs the admin role</CardTitle>
        </CardHeader>
        <CardContent className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
          This is an inventory of the deployment — which stores are wired, which services it reaches
          out to, which provider it trusts — so it sits with the operator’s routes rather than with
          the ones that help somebody read a run. Where an area you use is unavailable, its own
          screen says which variable is unset.
        </CardContent>
      </Card>
    );
  }
  return (
    <Refusal error={error} fallback="this instance did not answer what it is configured with" />
  );
}

function CapabilityRow({ capability }: { capability: SystemCapability }) {
  const configured = capability.state === 'configured';
  // A variable already printed beside the value it set is not repeated
  // underneath. Layout, not a decision: every variable the server named is on
  // the row either way, and the trailing line is for the ones whose value is
  // withheld — which is most of the interesting ones.
  const unsaid = capability.variables.filter(
    (variable) => !capability.settings.some((setting) => setting.variable === variable),
  );
  return (
    <div className="flex flex-col gap-2 py-3 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium">{capability.label}</span>
        <Badge tone={configured ? 'success' : 'neutral'}>
          {configured ? 'configured' : 'not configured'}
        </Badge>
        <span className="font-mono text-[11px] text-muted-foreground">{capability.id}</span>
      </div>

      {capability.settings.length > 0 ? (
        <dl className="flex flex-wrap gap-x-6 gap-y-1">
          {capability.settings.map((setting, at) => (
            <div key={`${setting.name}-${setting.value}-${at}`} className="flex items-baseline gap-1.5">
              <dt className="text-xs text-muted-foreground">{setting.name}</dt>
              <dd className="font-mono text-xs">{setting.value}</dd>
              {setting.variable ? (
                <span className="font-mono text-[11px] text-muted-foreground">
                  ({setting.variable})
                </span>
              ) : null}
            </div>
          ))}
        </dl>
      ) : null}

      <p className="max-w-3xl text-xs leading-relaxed text-muted-foreground">{capability.note}</p>

      {unsaid.length > 0 ? (
        <p className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
          <span>{configured ? 'Set by' : 'Set'}</span>
          {unsaid.map((variable) => (
            <code key={variable} className="rounded bg-muted px-1 py-0.5 font-mono">
              {variable}
            </code>
          ))}
        </p>
      ) : null}
    </div>
  );
}
