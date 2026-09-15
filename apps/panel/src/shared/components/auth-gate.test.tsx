import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/generated/client.gen';
import { AuthGate } from './auth-gate';
import { serve, withQueries } from '@/test/server';

beforeEach(() => client.setConfig({ baseUrl: 'http://panel.test' }));
afterEach(() => vi.unstubAllGlobals());

it('does not mount protected content when configuration fails, and recovers on retry', async () => {
  serve([{ method: 'GET', path: '/auth/config', answer: (call) => call <= 2
    ? { status: 503, body: { message: 'Unavailable' } }
    : { status: 200, body: { enabled: false, mode: 'none' } } }]);
  render(withQueries(<AuthGate><p>Protected work</p></AuthGate>));
  await screen.findByRole('alert', {}, { timeout: 3000 });
  expect(screen.queryByText('Protected work')).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByText('Protected work')).toBeTruthy();
});

it('requires a session when authentication is enabled', async () => {
  serve([
    { method: 'GET', path: '/auth/config', answer: { status: 200, body: { enabled: true, mode: 'oidc', provider: 'Authentik' } } },
    { method: 'GET', path: '/auth/me', answer: { status: 401 } },
  ]);
  render(withQueries(<AuthGate><p>Protected work</p></AuthGate>));
  expect(await screen.findByRole('button', { name: 'Sign in with Authentik' })).toBeTruthy();
  expect(screen.queryByText('Protected work')).toBeNull();
});
