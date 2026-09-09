import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { searchBlockLibrary, saveBlockTemplate } from '@/api/generated/sdk.gen';
import { createNotebook, getNotebookRevision } from '@/lib/ml-pipeline';
import { BlockLibrary } from './block-library';

vi.mock('@/api/generated/sdk.gen', () => ({
  searchBlockLibrary: vi.fn(),
  saveBlockTemplate: vi.fn(),
}));
vi.mock('@/lib/ml-pipeline', () => ({
  createNotebook: vi.fn(),
  getNotebookRevision: vi.fn(),
  getNotebook: vi.fn(),
}));

const solution = {
  id: 'external-solution',
  title: 'A solution supplied by the API',
  description: 'Custom preparation',
  tags: ['python'],
  spec: {
    kind: 'notebook' as const,
    notebook: 'shared',
    revision: 'a'.repeat(64),
    params: { limit: 2 },
  },
  revision: 'b'.repeat(64),
  saved_at: '2026-09-09T00:00:00Z',
};
function setup() {
  const onAdd = vi.fn();
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <BlockLibrary disabled={false} onAdd={onAdd} onBusyChange={vi.fn()} />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Public solutions library' }));
  return onAdd;
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(searchBlockLibrary).mockResolvedValue({
    response: new Response(null, { status: 200 }),
    data: { templates: [solution], total: 1, offset: 0, limit: 12 },
  } as never);
});

describe('public solutions library', () => {
  it('searches the API and copies the pinned notebook without modifying the public solution', async () => {
    vi.mocked(getNotebookRevision).mockResolvedValue({ source: 'exact pinned code' } as never);
    vi.mocked(createNotebook).mockResolvedValue({
      name: 'custom_copy',
      revision: 'c'.repeat(64),
    } as never);
    const onAdd = setup();
    await screen.findByText(solution.title);
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search public solutions' }), {
      target: { value: 'custom preparation' },
    });
    await waitFor(() =>
      expect(searchBlockLibrary).toHaveBeenLastCalledWith({
        query: { search: 'custom preparation', offset: 0, limit: 12 },
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Add copy' }));
    await waitFor(() => expect(onAdd).toHaveBeenCalled());
    expect(getNotebookRevision).toHaveBeenCalledWith('shared', 'a'.repeat(64));
    expect(createNotebook).toHaveBeenCalledWith('exact pinned code');
    expect(onAdd.mock.calls[0]?.[0].spec).toEqual({
      ...solution.spec,
      notebook: 'custom_copy',
      revision: 'c'.repeat(64),
    });
    expect(solution.spec.notebook).toBe('shared');
    expect(saveBlockTemplate).not.toHaveBeenCalled();
  });

  it('shows an unavailable library instead of substituting built-in solutions', async () => {
    vi.mocked(searchBlockLibrary).mockRejectedValue(new Error('Library is offline'));
    setup();
    await screen.findByRole('alert');
    expect(screen.getByRole('alert').textContent).toContain('Library is offline');
    expect(screen.queryByRole('button', { name: 'Add copy' })).toBeNull();
  });

  it('does not insert a notebook if copying its source fails', async () => {
    vi.mocked(getNotebookRevision).mockRejectedValue(new Error('Pinned source is unavailable'));
    const onAdd = setup();
    fireEvent.click(await screen.findByRole('button', { name: 'Add copy' }));
    await screen.findByRole('alert');
    expect(onAdd).not.toHaveBeenCalled();
    expect(createNotebook).not.toHaveBeenCalled();
  });
});
