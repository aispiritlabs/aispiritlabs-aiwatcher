import * as React from 'react';

import { EventFeed, type EventFeedEvent } from '@/features/observability/components/event-feed';
import { familyColor } from '@/features/observability/components/waterfall';
import {
  factsOf,
  otherAttributes,
  type Fact,
  type Span,
} from '@/features/observability/lib/span-facts';
import { PromptRefLink } from '@/shared/components/prompt-bits';
import { Badge, IdChip } from '@/shared/components/ui/primitives';
import {
  cn,
  formatCount,
  formatDuration,
  formatTime,
  formatUsd,
  shortId,
} from '@/shared/lib/utils';

/**
 * One span, read out.
 *
 * The waterfall answers "what took the time"; this answers everything else a
 * person arrives with — which model, on which settings, on which registered
 * prompt, what it spent, and what the producer actually sent to make it. The
 * last of those is why the raw events are at the bottom rather than a tab
 * away: a span is assembled from them (ADR_0003), and the assembly is the one
 * thing worth checking when a span looks wrong.
 *
 * Nothing here is fetched. Every fact is already in the run's payload and in
 * the events the page is streaming, so selecting a span costs no request and
 * works the same for a run still in flight.
 */
export function SpanDetail({
  span,
  events,
  everything,
  onClose,
}: {
  span: Span;
  events: EventFeedEvent[];
  /** Show the correlation ids too, from the view menu. */
  everything: boolean;
  onClose: () => void;
}) {
  const facts = React.useMemo(() => factsOf(span), [span]);
  const attributes = React.useMemo(() => otherAttributes(span, everything), [span, everything]);
  const startMs = Date.parse(span.start);
  const duration = Date.parse(span.end) - startMs;
  const failed = span.status.status === 'error';
  const firstToken = span.events?.find((event) => event.name === 'gen_ai.first_token');

  return (
    <div className="flex flex-col gap-4 p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex items-center gap-2">
            <span
              className={cn('h-2 w-2 shrink-0 rounded-full', familyColor[facts.family])}
              aria-hidden
            />
            <h3 className="truncate text-sm font-semibold" title={span.name}>
              {span.name}
            </h3>
            <Badge tone={failed ? 'danger' : 'neutral'}>{failed ? 'error' : span.kind}</Badge>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>span</span>
            <IdChip value={shortId(span.span_id)} full={span.span_id} label="span" />
            {span.parent_span_id ? (
              <>
                <span>· parent</span>
                <IdChip
                  value={shortId(span.parent_span_id)}
                  full={span.parent_span_id}
                  label="parent span"
                />
              </>
            ) : null}
          </div>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="shrink-0 rounded px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          Close
        </button>
      </div>

      {failed && span.status.message ? (
        <p className="rounded border border-danger/40 bg-danger/5 p-2 text-xs text-danger">
          {span.status.message}
        </p>
      ) : null}

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Measure label="Duration" value={formatDuration(duration)} />
        {firstToken ? (
          <Measure
            label="First token"
            value={formatDuration(Date.parse(firstToken.at) - startMs)}
          />
        ) : null}
        {facts.tokens ? (
          <Measure
            label="Tokens"
            value={formatCount(facts.tokens.input + facts.tokens.output)}
            hint={`${formatCount(facts.tokens.input)} in · ${formatCount(facts.tokens.output)} out`}
          />
        ) : null}
        {facts.tokens && facts.tokens.cached > 0 ? (
          <Measure label="Cached" value={formatCount(facts.tokens.cached)} />
        ) : null}
        {facts.costUsd !== undefined ? (
          <Measure label="Cost" value={formatUsd(facts.costUsd)} hint="as the provider billed it" />
        ) : null}
        <Measure label="Started" value={formatTime(span.start)} />
      </div>

      {facts.prompt ? (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span className="text-muted-foreground">Prompt</span>
          <PromptRefLink name={facts.prompt.name} versionId={facts.prompt.versionId} />
          {/* A claim about the request's text can only be made by a host that
              saw it — the gateway — so it is shown as that host's word rather
              than as a property of the call. */}
          {facts.prompt.verified === true ? <Badge tone="success">verified</Badge> : null}
          {facts.prompt.verified === false ? <Badge tone="warning">not found</Badge> : null}
          {facts.prompt.exact === true ? <Badge tone="neutral">exact</Badge> : null}
        </div>
      ) : null}

      {facts.groups.map((group) => (
        <Section key={group.title} title={group.title}>
          <dl className="grid grid-cols-1 gap-x-6 gap-y-1.5 sm:grid-cols-2">
            {group.facts.map((fact) => (
              <Row key={fact.label} fact={fact} />
            ))}
          </dl>
        </Section>
      ))}

      {span.events && span.events.length > 0 ? (
        <Section title="Span events">
          <ul className="flex flex-col gap-1">
            {span.events.map((event) => (
              <li
                key={`${event.name}-${event.at}`}
                className="flex items-baseline justify-between gap-3 text-xs"
              >
                <span>{event.name}</span>
                <span className="tabular-nums text-muted-foreground">
                  +{formatDuration(Date.parse(event.at) - startMs)}
                </span>
              </li>
            ))}
          </ul>
        </Section>
      ) : null}

      <Section title={`Attributes (${attributes.length})`}>
        {attributes.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            Everything this span carries is drawn above.
          </p>
        ) : (
          <dl className="grid grid-cols-1 gap-x-6 gap-y-1.5">
            {attributes.map(([key, value]) => (
              <Row key={key} fact={{ label: key, value }} />
            ))}
          </dl>
        )}
      </Section>

      <Section title={`Events (${events.length})`}>
        {events.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No event on the log carries this span id. A span the orphan sweeper closed is the usual
            reason.
          </p>
        ) : (
          <div className="-mx-4 border-y border-border">
            <EventFeed events={events} autoScroll={false} dense />
          </div>
        )}
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h4 className="text-xs uppercase tracking-wide text-muted-foreground">{title}</h4>
      {children}
    </section>
  );
}

function Row({ fact }: { fact: Fact }) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-border/30 pb-1 last:border-b-0">
      <dt className="shrink-0 text-xs text-muted-foreground">{fact.label}</dt>
      <dd className="min-w-0 break-all text-right text-xs">
        {fact.id ? <IdChip value={fact.value} label={fact.label} /> : fact.value}
      </dd>
    </div>
  );
}

function Measure({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-xs uppercase tracking-wide text-muted-foreground">{label}</span>
      <span className="text-sm font-semibold tabular-nums">{value}</span>
      {hint ? <span className="text-[11px] text-muted-foreground">{hint}</span> : null}
    </div>
  );
}
