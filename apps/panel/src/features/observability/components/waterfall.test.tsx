import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';

import { Waterfall } from './waterfall';
import type { Span } from '@/features/observability/lib/span-facts';

const agent: Span = {
  trace_id: 'trace',
  span_id: 'span-agent',
  name: 'invoke_agent planner-assistant',
  kind: 'internal',
  start: '2026-09-14T12:00:00.000Z',
  end: '2026-09-14T12:00:03.500Z',
  status: { status: 'ok' },
  attributes: [],
};

const call: Span = {
  trace_id: 'trace',
  span_id: 'span-llm',
  parent_span_id: 'span-agent',
  name: 'chat google/gemini-3.7-flash',
  kind: 'client',
  start: '2026-09-14T12:00:00.100Z',
  end: '2026-09-14T12:00:03.400Z',
  status: { status: 'ok' },
  attributes: [
    ['gen_ai.usage.input_tokens', 1_240],
    ['gen_ai.usage.output_tokens', 164],
  ],
};

it('opens the span that was clicked', async () => {
  const onSelect = vi.fn();
  render(<Waterfall spans={[call, agent]} onSelect={onSelect} />);

  await userEvent.click(screen.getByRole('button', { name: /chat google\/gemini-3.7-flash/ }));
  expect(onSelect).toHaveBeenCalledWith('span-llm');
});

it('marks the open span and counts what a call spent, which a bar cannot draw', () => {
  render(<Waterfall spans={[call, agent]} selected="span-llm" onSelect={() => {}} />);

  const rows = screen.getAllByRole('button');
  // The parent comes first even though the child was passed first: the tree is
  // built from `parent_span_id`, not from the order the spans arrived in.
  expect(rows[0]?.textContent).toContain('invoke_agent planner-assistant');
  expect(rows[1]?.getAttribute('aria-pressed')).toBe('true');
  expect(rows[1]?.textContent).toContain('1.2k → 164');
});

it('draws an orphan as a root rather than dropping it', () => {
  const orphan: Span = { ...call, parent_span_id: 'span-that-was-evicted' };
  render(<Waterfall spans={[orphan]} />);

  expect(screen.getAllByRole('button')).toHaveLength(1);
});
