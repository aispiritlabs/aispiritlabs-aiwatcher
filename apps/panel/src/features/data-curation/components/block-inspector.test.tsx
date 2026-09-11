import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { withQueries } from '@/test/server';
import {
  createNotebook,
  getNotebook,
  getNotebookRevision,
  getNotebooks,
  saveNotebook,
} from '@/shared/lib/ml-pipeline';
import { BlockInspector } from '@/features/data-curation/components/block-inspector';

vi.mock('@/shared/lib/ml-pipeline', () => ({
  createNotebook: vi.fn(),
  getNotebook: vi.fn(),
  getNotebookRevision: vi.fn(),
  getNotebooks: vi.fn(),
  saveNotebook: vi.fn(),
}));
const notebook = {
  name: 'original',
  source: '# pinned code',
  revision: 'a'.repeat(64),
  title: 'Original',
  size: 10,
  modified_at: '',
  app_url: '/ml-pipeline/app/original/',
};
const block = {
  id: 'python',
  title: 'My cell',
  spec: { kind: 'notebook' as const, notebook: notebook.name, revision: notebook.revision },
};
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getNotebooks).mockResolvedValue([notebook]);
  vi.mocked(getNotebookRevision).mockResolvedValue(notebook);
});

it('edits exactly the pinned source and reports unsaved code', async () => {
  const onDirtyChange = vi.fn();
  render(
    withQueries(
      <BlockInspector
        block={block}
        onChange={vi.fn()}
        onDelete={vi.fn()}
        onDirtyChange={onDirtyChange}
      />,
    ),
  );
  const code = await screen.findByLabelText('Code');
  await waitFor(() => expect((code as HTMLTextAreaElement).value).toBe('# pinned code'));
  expect(getNotebookRevision).toHaveBeenCalledWith('original', 'a'.repeat(64));
  expect(getNotebook).not.toHaveBeenCalled();
  await userEvent.type(code, '\n# changed');
  await waitFor(() => expect(onDirtyChange).toHaveBeenLastCalledWith('python', true));
  expect((screen.getByLabelText('Notebook') as HTMLSelectElement).disabled).toBe(true);
  await userEvent.click(screen.getByRole('button', { name: 'Discard code edits' }));
  await waitFor(() => expect(onDirtyChange).toHaveBeenLastCalledWith('python', false));
  expect((code as HTMLTextAreaElement).value).toBe('# pinned code');
});

it('saves an edited copy into an independent notebook without overwriting the original', async () => {
  const onChange = vi.fn();
  vi.mocked(createNotebook).mockResolvedValue({
    ...notebook,
    name: 'independent',
    revision: 'b'.repeat(64),
  });
  render(withQueries(<BlockInspector block={block} onChange={onChange} onDelete={vi.fn()} />));
  const code = await screen.findByLabelText('Code');
  await waitFor(() => expect((code as HTMLTextAreaElement).value).toBe('# pinned code'));
  await userEvent.clear(code);
  await userEvent.type(code, '# my new code');
  await userEvent.click(screen.getByRole('button', { name: 'Save as copy' }));
  await waitFor(() => expect(createNotebook).toHaveBeenCalledWith('# my new code'));
  expect(saveNotebook).not.toHaveBeenCalled();
  expect(onChange).toHaveBeenLastCalledWith({
    ...block,
    spec: { ...block.spec, notebook: 'independent', revision: 'b'.repeat(64) },
  });
});
