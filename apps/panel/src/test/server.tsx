import * as React from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { vi } from 'vitest';

/**
 * A server that answers exactly what a test asks it to.
 *
 * The stub is `fetch`, not the generated SDK, so everything under test is the
 * real path: the generated client's own decision to *resolve* on a 403 rather
 * than throw is what review R6 is about, and mocking the SDK would replace
 * precisely the thing that has to be proved.
 */

export type Answer = { status: number; body?: unknown };

/** One route: a method, a path suffix, and what the server says to it. */
export type Route = {
  method: string;
  path: string;
  answer: Answer | ((call: number) => Answer);
};

export type Server = {
  /** Every request that reached it, in order. */
  readonly calls: { method: string; url: string }[];
  /** How many times one route was asked. */
  countOf: (method: string, path: string) => number;
};

export function serve(routes: Route[]): Server {
  const calls: { method: string; url: string }[] = [];
  const seen = new Map<Route, number>();

  vi.stubGlobal('fetch', async (input: Request | string, init?: RequestInit) => {
    // Read rather than reconstructed: the generated client hands over a
    // `Request`, and `lib/ml-pipeline.ts` hands over a *relative* string, which
    // `new Request` refuses outright outside a browser.
    const raw = input instanceof Request ? input.url : String(input);
    const method = (input instanceof Request ? input.method : init?.method) ?? 'GET';
    const url = new URL(raw, 'http://panel.test');
    calls.push({ method, url: url.pathname });

    const route = routes.find(
      (candidate) => candidate.method === method && url.pathname.endsWith(candidate.path),
    );
    if (!route) {
      return jsonResponse(500, {
        code: 'no_such_route',
        message: `nothing serves ${method} ${url.pathname}`,
      });
    }

    const count = (seen.get(route) ?? 0) + 1;
    seen.set(route, count);
    const answer = typeof route.answer === 'function' ? route.answer(count) : route.answer;
    return jsonResponse(answer.status, answer.body);
  });

  return {
    calls,
    countOf: (method, path) =>
      calls.filter((call) => call.method === method && call.url.endsWith(path)).length,
  };
}

function jsonResponse(status: number, body: unknown): Response {
  // 204 carries no body, which is the whole point of the DELETE case: the
  // client turns an empty response into `{}`, so "is there data" answers yes
  // for a refusal and yes for a success alike.
  if (status === 204 || body === undefined) return new Response(null, { status });
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

/** The body the API returns for every refusal. */
export function refusal(code: string, message: string, details?: string[]) {
  return details ? { code, message, details } : { code, message };
}

/**
 * A provider with retries off, so a test asserts on the first answer rather
 * than on the fourth — and with no cache shared between tests.
 */
export function withQueries(ui: React.ReactNode) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
      mutations: { retry: false },
    },
  });
  return <QueryClientProvider client={client}>{ui}</QueryClientProvider>;
}
