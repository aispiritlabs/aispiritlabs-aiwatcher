import { QueryClient } from '@tanstack/react-query';
import { createMemoryHistory, createRouter } from '@tanstack/react-router';
import { describe, expect, it } from 'vitest';

import { routeTree } from '@/routeTree.gen';

async function open(href: string) {
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [href] }),
    context: { queryClient: new QueryClient() },
  });
  await router.load();
  return router;
}

describe('feature route integration', () => {
  it.each([
    '/annotations/label',
    '/annotations/sources',
    '/annotations/imports',
    '/annotations/exports',
    '/conversations/review',
    '/conversations/corpora',
    '/data-curation/pipeline',
    '/data-curation/recipe',
    '/datasets',
    '/account',
    '/account/access',
    '/learning',
    '/evaluation',
    '/experiments',
    '/observability/explore',
    '/observability/live',
    '/observability/metrics',
    '/observability/query',
    '/observability/runs',
    '/prompts',
    '/prompts/example',
    '/runs/example',
    '/training/models',
    '/training/runs',
    '/workflows',
  ])('resolves the existing deep link %s', async (href) => {
    const router = await open(href);
    expect(router.state.matches.at(-1)?.pathname.replace(/\/$/, '')).toBe(href);
    expect(router.state.matches.every((match) => match.status === 'success')).toBe(true);
  });

  it.each([
    ['/observability', '/observability/explore'],
    ['/annotations', '/annotations/label'],
    ['/conversations', '/conversations/review'],
    ['/data-curation', '/data-curation/pipeline'],
    ['/training', '/training/runs'],
  ])('keeps the redirect from %s to %s', async (from, to) => {
    const router = await open(from);
    expect(router.state.location.pathname).toBe(to);
  });

  it('keeps the organization and project being administered in the URL', async () => {
    const router = await open('/account/access?organization=org-1&project=project-1');
    expect(router.state.matches.at(-1)?.search).toEqual({
      organization: 'org-1',
      project: 'project-1',
    });
  });

  it('carries the project being read from one area to the next', async () => {
    // `NavArea.carries` keeps a period inside Observability and an annotation
    // project inside Annotations, and drops each at the area's edge. A scope
    // belongs to no area: it decides which rows exist at all, so it survives
    // every move, and the root route's middleware is what makes that true for
    // every link in the panel rather than for the ones somebody remembered.
    const router = await open('/observability/runs?scope=org-1/proj-1');
    expect(router.state.location.search).toEqual({ scope: 'org-1/proj-1' });
    await router.navigate({ to: '/workflows' });
    expect(router.state.location.search).toEqual({ scope: 'org-1/proj-1' });
    await router.navigate({ to: '/datasets', search: { scope: undefined } });
    expect(router.state.location.search).toEqual({});
  });

  it('opens a neutral workspace at the root', async () => {
    const router = await open('/');
    expect(router.state.location.pathname).toBe('/');
    expect(router.state.matches.at(-1)?.status).toBe('success');
  });

  it('preserves a shared pipeline selection through the extracted URL contract', async () => {
    const router = await open(
      '/data-curation/pipeline?name=demo&block=source&reach=both&phase=ml&view=notebook&window=3600&execution=run-1',
    );
    expect(router.state.matches.at(-1)?.search).toEqual({
      name: 'demo',
      block: 'source',
      reach: 'both',
      phase: 'ml',
      view: 'notebook',
      window: 3600,
      execution: 'run-1',
    });
  });

  it('still rejects invalid selection values', async () => {
    const router = await open('/data-curation/pipeline?reach=invalid&window=-1');
    expect(router.state.matches.at(-1)?.status).toBe('error');
    expect(router.state.matches.at(-1)?.searchError).toBeDefined();
  });
});
