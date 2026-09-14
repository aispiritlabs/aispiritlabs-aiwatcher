import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';

import { SpanDetail } from './span-detail';
import type { Span } from '@/features/observability/lib/span-facts';

// `PromptRefLink` links into the prompt registry, and a `Link` outside a router
// throws. What matters here is that the reference is drawn at all.
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="#prompt">{children}</a>,
}));

const call: Span = {
  trace_id: 'trace',
  span_id: 'span-llm',
  parent_span_id: 'span-agent',
  name: 'chat google/gemini-3.7-flash',
  kind: 'client',
  start: '2026-09-14T12:00:00.000Z',
  end: '2026-09-14T12:00:03.400Z',
  status: { status: 'ok' },
  attributes: [
    ['gen_ai.provider.name', 'openrouter.ai'],
    ['gen_ai.request.model', 'google/gemini-3.7-flash'],
    ['gen_ai.request.temperature', 0.2],
    ['gen_ai.usage.input_tokens', 1_240],
    ['gen_ai.usage.output_tokens', 164],
    ['aiwatcher.prompt.name', 'planner.assistant'],
    ['aiwatcher.prompt.version_id', 'b'.repeat(64)],
    ['messaging.message.id', 'event-7'],
    ['planner.owner', 'local-developer'],
  ],
  events: [{ name: 'gen_ai.first_token', at: '2026-09-14T12:00:00.900Z' }],
};

const events = [
  {
    checkpoint: '000000000001',
    span_id: 'span-llm',
    event_type: 'llm.completed',
    occurred_at: '2026-09-14T12:00:03.400Z',
    data: { model: 'google/gemini-3.7-flash', output_tokens: 164 },
  },
];

it('reads a model call out instead of leaving it in the payload', () => {
  render(<SpanDetail span={call} events={events} everything={false} onClose={() => {}} />);

  expect(screen.getByText('openrouter.ai')).toBeTruthy();
  expect(screen.getByText('google/gemini-3.7-flash')).toBeTruthy();
  expect(screen.getByText('0.2')).toBeTruthy();
  // Tokens, and the first token's arrival timed off the span's own start.
  expect(screen.getByText('1.4k')).toBeTruthy();
  expect(screen.getByText('900 ms')).toBeTruthy();
  expect(screen.getByRole('link', { name: /planner.assistant/ })).toBeTruthy();
});

it('counts the events the span was assembled from and feeds them in', () => {
  render(<SpanDetail span={call} events={events} everything={false} onClose={() => {}} />);

  // The feed itself is virtualised, so jsdom mounts no rows for it; what this
  // holds is that the span's own events reached it rather than the run's.
  expect(screen.getByText('Events (1)')).toBeTruthy();
  expect(screen.getByText('Payload')).toBeTruthy();
});

it('keeps a producer attribute and holds the correlation ids behind the view menu', () => {
  const { rerender } = render(
    <SpanDetail span={call} events={events} everything={false} onClose={() => {}} />,
  );

  expect(screen.getByText('planner.owner')).toBeTruthy();
  expect(screen.queryByText('messaging.message.id')).toBeNull();

  rerender(<SpanDetail span={call} events={events} everything onClose={() => {}} />);
  expect(screen.getByText('messaging.message.id')).toBeTruthy();
});

it('says why a span carries no event rather than showing an empty table', () => {
  render(<SpanDetail span={call} events={[]} everything={false} onClose={() => {}} />);

  expect(screen.getByText(/No event on the log carries this span id/)).toBeTruthy();
});

it('closes on request, because the row that opened it is above the fold', async () => {
  const onClose = vi.fn();
  render(<SpanDetail span={call} events={events} everything={false} onClose={onClose} />);

  await userEvent.click(screen.getByRole('button', { name: 'Close' }));
  expect(onClose).toHaveBeenCalled();
});
