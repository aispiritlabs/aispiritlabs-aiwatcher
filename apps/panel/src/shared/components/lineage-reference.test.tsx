import { render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DatasetReference, ExecutionReference } from './lineage-reference';
import { serve, withQueries } from '@/test/server';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, search }: { children: React.ReactNode; to: string; search: unknown }) => (
    <a href={to} data-search={JSON.stringify(search)}>{children}</a>
  ),
}));
afterEach(() => vi.unstubAllGlobals());

it('links a confirmed annotation export to annotations, never the curation registry', async () => {
  serve([
    { method: 'GET', path: '/datasets', answer: { status: 200, body: { datasets: [] } } },
    {
      method: 'GET',
      path: '/annotation-exports',
      answer: { status: 200, body: { exports: [{ export: 'v1' }] } },
    },
  ]);
  render(withQueries(<DatasetReference reference="project@v1" />));
  const link = await screen.findByRole('link');
  expect(link.getAttribute('href')).toBe('/annotations/exports');
});

it('does not guess a registry for an unversioned or externally typed source', () => {
  const server = serve([]);
  render(
    withQueries(
      <>
        <DatasetReference reference="project" />
        <DatasetReference reference="project@v1" kind="external" />
        <DatasetReference reference="project@v1" version="v2" kind="annotations" />
      </>,
    ),
  );
  expect(screen.queryByRole('link')).toBeNull();
  expect(server.calls).toHaveLength(0);
});

it('links an explicitly identified curation version with its exact target', async () => {
  serve([
    { method: 'GET', path: '/datasets', answer: { status: 200, body: {
      datasets: [{ name: 'features', versions: [{ version: 'v2' }] }],
    } } },
  ]);
  render(withQueries(<DatasetReference reference="features" kind="curation" version="v2" />));
  const link = await screen.findByRole('link');
  expect(link.getAttribute('href')).toBe('/datasets');
  expect(JSON.parse(link.getAttribute('data-search')!)).toEqual({ dataset: 'features', version: 'v2' });
});

it('links an execution and step only after their retained identity is confirmed', async () => {
  serve([
    { method: 'GET', path: '/workflow-executions/run-1', answer: { status: 200, body: {
      summary: { workflow_id: 'evaluation-flow' }, nodes: [{ node_id: 'score' }],
    } } },
  ]);
  render(withQueries(<ExecutionReference executionId="run-1" stepId="score" />));
  const link = await screen.findByRole('link');
  expect(link.getAttribute('href')).toBe('/workflows');
  expect(JSON.parse(link.getAttribute('data-search')!)).toEqual({
    workflow: 'evaluation-flow', execution: 'run-1', node: 'score', window: 0,
  });
});
