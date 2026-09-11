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
    ['/', '/observability/explore'],
    ['/observability', '/observability/explore'],
    ['/annotations', '/annotations/label'],
    ['/conversations', '/conversations/review'],
    ['/data-curation', '/data-curation/pipeline'],
    ['/training', '/training/runs'],
  ])('keeps the redirect from %s to %s', async (from, to) => {
    const router = await open(from);
    expect(router.state.location.pathname).toBe(to);
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
