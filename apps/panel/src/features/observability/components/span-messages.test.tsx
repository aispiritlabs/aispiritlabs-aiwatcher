import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { SpanMessages } from './span-messages';
import { refusal, serve, withQueries } from '@/test/server';

afterEach(() => vi.unstubAllGlobals());

const turn = {
  turn_id: 'turn-1',
  conversation_id: 'conversation:1',
  message_id: 'm1',
  ordinal: 0,
  role: 'user',
  content_digest: 'a'.repeat(64),
  content_bytes: 42,
  state: 'held',
  policy: {},
  review: { state: 'pending' },
};

it('asks the archive only for the turns this call produced', async () => {
  const server = serve([
    {
      method: 'GET',
      path: '/conversation-turns',
      answer: { status: 200, body: { turns: [turn], total: 1 } },
    },
  ]);
  render(
    withQueries(
      <SpanMessages conversationId="conversation:1" runId="run-1" spanId="span-1" reveal={false} />,
    ),
  );

  expect(await screen.findByText('user')).toBeTruthy();
  const asked = server.calls.at(-1);
  expect(asked?.url).toContain('/conversation-turns');
  // The words are a second request, and nothing has asked for them yet.
  expect(server.countOf('GET', '/conversation-turn-content')).toBe(0);
  expect(screen.getByRole('button', { name: /Show what was said/ })).toBeTruthy();
});

it('reads a deployment with no archive as a configuration, not a failure', async () => {
  serve([
    {
      method: 'GET',
      path: '/conversation-turns',
      answer: {
        status: 501,
        body: refusal('registry_disabled', 'AIWATCHER_CONVERSATION_ARCHIVE is unset'),
      },
    },
  ]);
  render(
    withQueries(
      <SpanMessages conversationId="conversation:1" runId="run-1" spanId="span-1" reveal={false} />,
    ),
  );

  expect(await screen.findByText(/No content archive on this deployment/)).toBeTruthy();
});

it('says the archive holds nothing rather than showing an empty list', async () => {
  serve([
    {
      method: 'GET',
      path: '/conversation-turns',
      answer: { status: 200, body: { turns: [], total: 0 } },
    },
  ]);
  render(
    withQueries(
      <SpanMessages conversationId="conversation:1" runId="run-1" spanId="span-1" reveal={false} />,
    ),
  );

  expect(await screen.findByText(/holds nothing for this call/)).toBeTruthy();
});

it('opens what was said on request, and says so when the role refuses', async () => {
  serve([
    {
      method: 'GET',
      path: '/conversation-turns',
      answer: { status: 200, body: { turns: [turn], total: 1 } },
    },
    {
      method: 'GET',
      path: '/conversation-turn-content',
      answer: { status: 403, body: refusal('forbidden', 'reading content needs the admin role') },
    },
  ]);
  render(
    withQueries(
      <SpanMessages conversationId="conversation:1" runId="run-1" spanId="span-1" reveal={false} />,
    ),
  );

  await userEvent.click(await screen.findByRole('button', { name: /Show what was said/ }));
  expect(await screen.findByText(/needs the admin role/)).toBeTruthy();
});

it('opens every message at once when the view menu asks for it', async () => {
  serve([
    {
      method: 'GET',
      path: '/conversation-turns',
      answer: { status: 200, body: { turns: [turn], total: 1 } },
    },
    {
      method: 'GET',
      path: '/conversation-turn-content',
      answer: { status: 200, body: { parts: [{ kind: 'text', text: 'what should I do next' }] } },
    },
  ]);
  render(
    withQueries(
      <SpanMessages conversationId="conversation:1" runId="run-1" spanId="span-1" reveal />,
    ),
  );

  expect(await screen.findByText('what should I do next')).toBeTruthy();
});
