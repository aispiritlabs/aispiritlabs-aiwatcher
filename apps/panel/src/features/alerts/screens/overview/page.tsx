import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate, useSearch } from '@tanstack/react-router';

import {
  getAlertChannel,
  listAlertDeliveries,
  listAlertRules,
  retryAlertDelivery,
  setAlertRuleEnabled,
  testAlertChannel,
} from '@/api/generated';
import type { AlertChannelTest, AlertDelivery, AlertRuleSummary, JobState } from '@/api/generated';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Refusal,
  Spinner,
} from '@/shared/components/ui/primitives';
import { ApiFailure, answerOf } from '@/shared/lib/result';

/**
 * What is worth telling somebody about, and what was actually sent.
 *
 * The one area here that speaks first. Everything else waits to be opened; a
 * run that failed at four in the morning is a row somebody reads at nine.
 *
 * Three things on this screen are the server's and are not worked out here.
 * **Whether a notification arrived** is the delivery's own state and its
 * attempt count, never inferred from an age or a spinner. **What a receiver
 * saw** is the payload stored beside it, shown as it was sent rather than
 * rebuilt from the rule. And **what happens next** to a failed one is the
 * server's too: Send again is a request, and the row it returns is what the
 * queue thinks, not what this hoped.
 *
 * Two absences are drawn as sentences rather than as failures. Reading this
 * needs the `admin` role, because a rule decides who gets woken; and a
 * deployment with no channel configured is a working state — rules are kept,
 * regressions are still found, and what does not happen is anybody being told.
 * The screen says which of the two it is instead of rendering an empty list.
 */
const STATES: Array<{ id: JobState; label: string }> = [
  { id: 'queued', label: 'waiting' },
  { id: 'completed', label: 'delivered' },
  { id: 'failed', label: 'given up on' },
];

function toneOf(state: JobState): 'success' | 'danger' | 'neutral' {
  if (state === 'completed') return 'success';
  if (state === 'failed') return 'danger';
  return 'neutral';
}

function labelOf(state: JobState): string {
  return STATES.find((known) => known.id === state)?.label ?? state;
}

function when(at: number | null | undefined): string {
  if (at === null || at === undefined) return '—';
  return new Date(at * 1000).toLocaleString();
}

export function AlertsPage() {
  const search = useSearch({ from: '/alerts' });
  const navigate = useNavigate({ from: '/alerts' });
  const client = useQueryClient();

  const channel = useQuery({
    queryKey: ['alert-channel'],
    retry: false,
    queryFn: async () =>
      answerOf(await getAlertChannel(), 'this instance did not say where an alert goes'),
  });

  const rules = useQuery({
    queryKey: ['alert-rules'],
    retry: false,
    queryFn: async () =>
      answerOf(await listAlertRules({ query: { limit: 200 } }), 'the alert rules did not load'),
  });

  const deliveries = useQuery({
    queryKey: ['alert-deliveries', search.rule ?? null, search.state ?? null],
    retry: false,
    queryFn: async () =>
      answerOf(
        await listAlertDeliveries({
          query: { rule: search.rule, state: search.state, limit: 100 },
        }),
        'what was sent did not load',
      ),
  });

  const test = useMutation({
    mutationFn: async (): Promise<AlertChannelTest> =>
      answerOf(await testAlertChannel(), 'the channel test did not answer'),
    onSuccess: () => void client.invalidateQueries({ queryKey: ['alert-deliveries'] }),
  });

  const retry = useMutation({
    mutationFn: async (key: string) =>
      answerOf(
        await retryAlertDelivery({ path: { key } }),
        'this notification could not be sent again',
      ),
    onSuccess: () => void client.invalidateQueries({ queryKey: ['alert-deliveries'] }),
  });

  const silence = useMutation({
    mutationFn: async ({ name, enabled }: { name: string; enabled: boolean }) =>
      answerOf(
        await setAlertRuleEnabled({ path: { name }, body: { enabled } }),
        'this rule could not be switched',
      ),
    onSuccess: () => void client.invalidateQueries({ queryKey: ['alert-rules'] }),
  });

  const refused = rules.error ?? deliveries.error ?? channel.error;
  if (refused instanceof ApiFailure && (refused.status === 403 || refused.status === 501)) {
    return <Unavailable error={refused} />;
  }

  return (
    <div className="flex max-w-5xl flex-col gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">Alerts</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            What this deployment says out loud: a managed run that died, and a measurement that got
            worse than the one before it. One channel, set where the process is started.
          </p>
        </div>
      </div>

      <Card>
        <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2">
          <div>
            <CardTitle>Where it goes</CardTitle>
            <p className="text-xs text-muted-foreground">
              A destination is configuration, never something a rule carries.
            </p>
          </div>
          <Button
            size="sm"
            variant="outline"
            disabled={!channel.data?.channel || test.isPending}
            onClick={() => test.mutate()}
          >
            {test.isPending ? 'Sending…' : 'Send a test'}
          </Button>
        </CardHeader>
        <CardContent className="flex flex-col gap-2 text-sm">
          {channel.isPending ? (
            <p className="flex items-center gap-2 text-muted-foreground">
              <Spinner /> Reading what this instance sends through…
            </p>
          ) : null}
          {channel.data?.channel ? (
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone="success">{channel.data.channel.kind}</Badge>
              <Badge tone={channel.data.channel.signed ? 'success' : 'neutral'}>
                {channel.data.channel.signed ? 'signed' : 'unsigned'}
              </Badge>
              <span className="text-xs text-muted-foreground">
                set by{' '}
                <code className="rounded bg-muted px-1 py-0.5 font-mono">
                  {channel.data.channel.variable}
                </code>
              </span>
            </div>
          ) : null}
          {channel.data && !channel.data.channel ? (
            <p className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
              Nothing is configured, so nothing is sent. Rules are still kept and a regression is
              still found — set{' '}
              <code className="rounded bg-muted px-1 py-0.5 font-mono">
                AIWATCHER_ALERT_WEBHOOK_URL
              </code>{' '}
              to have somebody told.
            </p>
          ) : null}
          {test.data ? <TestOutcome outcome={test.data} /> : null}
          {test.error ? (
            <Refusal error={test.error} fallback="the channel test did not answer" />
          ) : null}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Rules</CardTitle>
          <p className="text-xs text-muted-foreground">
            Switching one off is not an edit: the same occurrence is not sent twice when it comes
            back on.
          </p>
        </CardHeader>
        <CardContent className="flex flex-col divide-y divide-border">
          {rules.isPending ? (
            <p className="flex items-center gap-2 py-3 text-sm text-muted-foreground">
              <Spinner /> Reading the rules…
            </p>
          ) : null}
          {rules.data?.rules.length === 0 ? (
            <EmptyState
              title="No rule has been published"
              hint="A rule names what is worth telling somebody about — a definition whose runs must not die quietly, or a context whose measurements must not get worse."
            />
          ) : null}
          {rules.data?.rules.map((rule) => (
            <RuleRow
              key={rule.name}
              rule={rule}
              busy={silence.isPending}
              onSwitch={(enabled) => silence.mutate({ name: rule.name, enabled })}
              onShow={() =>
                void navigate({
                  search: (was) => ({
                    ...was,
                    rule: was.rule === rule.name ? undefined : rule.name,
                  }),
                })
              }
              showing={search.rule === rule.name}
            />
          ))}
          {rules.error && !(rules.error instanceof ApiFailure && rules.error.status === 403) ? (
            <Refusal error={rules.error} fallback="the alert rules did not load" />
          ) : null}
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2">
          <div>
            <CardTitle>What was sent</CardTitle>
            <p className="text-xs text-muted-foreground">
              Newest first. Every attempt is counted, and a repeat carries the key the receiver
              deduplicates by.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-1">
            {STATES.map((state) => (
              <Button
                key={state.id}
                size="sm"
                variant={search.state === state.id ? 'default' : 'outline'}
                onClick={() =>
                  void navigate({
                    search: (was) => ({
                      ...was,
                      state: was.state === state.id ? undefined : state.id,
                    }),
                  })
                }
              >
                {state.label}
              </Button>
            ))}
          </div>
        </CardHeader>
        <CardContent className="flex flex-col divide-y divide-border">
          {deliveries.isPending ? (
            <p className="flex items-center gap-2 py-3 text-sm text-muted-foreground">
              <Spinner /> Reading what was sent…
            </p>
          ) : null}
          {deliveries.data?.deliveries.length === 0 ? (
            <EmptyState
              title="Nothing has been sent"
              hint="A notification appears here the moment one is created, before the channel has taken it."
            />
          ) : null}
          {deliveries.data?.deliveries.map((delivery) => (
            <DeliveryRow
              key={delivery.dedup_key}
              delivery={delivery}
              busy={retry.isPending}
              onRetry={() => retry.mutate(delivery.dedup_key)}
            />
          ))}
          {retry.error ? (
            <Refusal error={retry.error} fallback="this notification could not be sent again" />
          ) : null}
          {deliveries.error &&
          !(deliveries.error instanceof ApiFailure && deliveries.error.status === 403) ? (
            <Refusal error={deliveries.error} fallback="what was sent did not load" />
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}

/**
 * A read this caller may not make, in the two shapes it comes in.
 *
 * A 403 is a role and a 501 is a deployment with no object store — neither is
 * something going wrong, and drawing either as an empty list would say this
 * deployment has no rules when it may have several.
 */
function Unavailable({ error }: { error: ApiFailure }) {
  const role = error.status === 403;
  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <h1 className="text-lg font-semibold">Alerts</h1>
      <Card className="border-dashed">
        <CardHeader>
          <CardTitle className="text-sm">
            {role
              ? 'Reading what this deployment sends needs the admin role'
              : 'This deployment keeps no alert rules'}
          </CardTitle>
        </CardHeader>
        <CardContent className="text-xs leading-relaxed text-muted-foreground">
          {role ? (
            <>
              A rule decides who is woken at four in the morning, so it sits with the operator’s
              routes rather than with the ones that help somebody read a run.
            </>
          ) : (
            <>
              A rule is authored data and lives in the object store, so this area needs the same one
              Prompts does. Set{' '}
              <code className="rounded bg-muted px-1 py-0.5 font-mono">AIWATCHER_PROMPT_STORE</code>{' '}
              to have somewhere to keep them.
            </>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function TestOutcome({ outcome }: { outcome: AlertChannelTest }) {
  return (
    <div className="flex flex-col gap-1 rounded border border-border p-2">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={outcome.delivered ? 'success' : 'danger'}>
          {outcome.delivered ? 'the receiver took it' : 'the receiver did not take it'}
        </Badge>
        <span className="font-mono text-[11px] text-muted-foreground">
          {outcome.delivery.dedup_key.slice(0, 12)}
        </span>
      </div>
      {outcome.error ? <p className="text-xs text-destructive">{outcome.error}</p> : null}
    </div>
  );
}

function RuleRow({
  rule,
  busy,
  showing,
  onSwitch,
  onShow,
}: {
  rule: AlertRuleSummary;
  busy: boolean;
  showing: boolean;
  onSwitch: (enabled: boolean) => void;
  onShow: () => void;
}) {
  return (
    <div className="flex flex-col gap-1.5 py-3 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium">{rule.name}</span>
        <Badge tone={rule.enabled ? 'success' : 'neutral'}>
          {rule.enabled ? 'on' : 'silenced'}
        </Badge>
        {rule.current ? <Badge tone="neutral">{rule.current.trigger}</Badge> : null}
        <span className="ml-auto flex items-center gap-1">
          <Button size="sm" variant="outline" onClick={onShow}>
            {showing ? 'All rules' : 'What it sent'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => onSwitch(!rule.enabled)}
          >
            {rule.enabled ? 'Silence' : 'Switch on'}
          </Button>
        </span>
      </div>
      {rule.current ? (
        <p className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
          {rule.current.description}
        </p>
      ) : null}
      <p className="text-[11px] text-muted-foreground">
        {rule.versions} version{rule.versions === 1 ? '' : 's'} · changed {when(rule.updated_at)}
      </p>
    </div>
  );
}

function DeliveryRow({
  delivery,
  busy,
  onRetry,
}: {
  delivery: AlertDelivery;
  busy: boolean;
  onRetry: () => void;
}) {
  return (
    <div className="flex flex-col gap-1.5 py-3 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium">{delivery.payload.title}</span>
        <Badge tone={toneOf(delivery.state)}>{labelOf(delivery.state)}</Badge>
        <span className="text-[11px] text-muted-foreground">
          {delivery.attempts} attempt{delivery.attempts === 1 ? '' : 's'}
        </span>
        {delivery.state === 'failed' ? (
          <Button size="sm" variant="outline" className="ml-auto" disabled={busy} onClick={onRetry}>
            Send again
          </Button>
        ) : null}
      </div>

      <dl className="flex flex-wrap gap-x-6 gap-y-1">
        {delivery.payload.facts?.map((fact, at) => (
          <div key={`${fact.label}-${at}`} className="flex items-baseline gap-1.5">
            <dt className="text-xs text-muted-foreground">{fact.label}</dt>
            <dd className="font-mono text-xs">{fact.value}</dd>
          </div>
        ))}
      </dl>

      {delivery.last_error ? (
        <p className="max-w-3xl text-xs text-destructive">{delivery.last_error}</p>
      ) : null}

      <p className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
        <span>{delivery.rule}</span>
        <span>·</span>
        <span>raised {when(delivery.created_at)}</span>
        {delivery.delivered_at ? (
          <>
            <span>·</span>
            <span>delivered {when(delivery.delivered_at)}</span>
          </>
        ) : null}
        {delivery.state === 'queued' && delivery.attempts > 0 ? (
          <>
            <span>·</span>
            <span>next try {when(delivery.available_at)}</span>
          </>
        ) : null}
        <span>·</span>
        <code className="font-mono">{delivery.dedup_key.slice(0, 12)}</code>
      </p>
    </div>
  );
}
