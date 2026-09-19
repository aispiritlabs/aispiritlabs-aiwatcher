import { readFileSync } from 'node:fs';
import path from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import {
  SCOPED_ROUTES,
  formatScope,
  parseScope,
  scopedPath,
  setActiveScope,
} from '@/shared/lib/scope';

const CONTRACT = path.resolve(import.meta.dirname, '../../../../../contracts/openapi.json');
const SCOPED = '/api/v1/orgs/{organization}/projects/{project}/';

/** Every instance route the server also serves under a project, from the contract. */
function twinsInTheContract(): string[] {
  const document = JSON.parse(readFileSync(CONTRACT, 'utf8')) as { paths: Record<string, unknown> };
  const paths = new Set(Object.keys(document.paths));
  return [...paths]
    .filter((route) => route.startsWith(SCOPED))
    .map((route) => `/api/v1/${route.slice(SCOPED.length)}`)
    .filter((route) => paths.has(route))
    .sort();
}

afterEach(() => setActiveScope(undefined));

describe('which reads can answer for one project', () => {
  it('names exactly the routes the server serves under a project, and no others', () => {
    // The one test that fails on the *day the boundary moves*, which is the
    // point: the panel says in `navigation.ts` which areas a selected project
    // reaches into, and that claim is only as true as this list. A fold that
    // gains a project grows the contract, this list stops matching, and
    // whoever moved it is told which area to re-describe.
    expect([...SCOPED_ROUTES].sort()).toEqual(twinsInTheContract());
  });

  it('leaves a route with no project twin exactly as it was', () => {
    // Not a rewrite to a path that does not exist: that answers 404, which
    // reads as "you may not see this" when the truth is "this is not a
    // project's to see".
    const scope = { organization: 'org', project: 'proj' };
    expect(scopedPath('/api/v1/workflows', '/api/v1/workflows', scope)).toBe('/api/v1/workflows');
    expect(scopedPath('/api/v1/evaluations', '/api/v1/evaluations', scope)).toBe(
      '/api/v1/evaluations',
    );
  });

  it('puts the project in front of the route the contract names, path parameters and all', () => {
    const scope = { organization: 'org', project: 'proj' };
    expect(scopedPath('/api/v1/runs/run-1/stream', '/api/v1/runs/{run_id}/stream', scope)).toBe(
      '/api/v1/orgs/org/projects/proj/runs/run-1/stream',
    );
  });

  it('answers the unassigned side when no project is selected', () => {
    expect(scopedPath('/api/v1/runs', '/api/v1/runs', undefined)).toBe('/api/v1/runs');
  });
});

describe('a scope in the URL', () => {
  it('round-trips the spelling the log and the ingest token use', () => {
    expect(parseScope(formatScope({ organization: 'a', project: 'b' }))).toEqual({
      organization: 'a',
      project: 'b',
    });
  });

  it('refuses anything that is not one organization and one project', () => {
    for (const value of ['', 'a', 'a/', '/b', 'a/b/c']) {
      expect(parseScope(value)).toBeUndefined();
    }
  });
});
