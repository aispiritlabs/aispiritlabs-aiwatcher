import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LocalViews } from './local-views';
import { viewFromSearch } from '@/shared/lib/local-views';

const identity = vi.hoisted(() => ({ scope: 'instance:alice' as string | undefined }));
vi.mock('@/shared/lib/local-views', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/shared/lib/local-views')>()),
  useLocalScope: () => identity.scope,
}));

beforeEach(() => {
  identity.scope = 'instance:alice';
  const data = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => data.set(key, value),
  });
});
afterEach(() => vi.unstubAllGlobals());

it('separates identities, disables unknown identity, and restores the explicit comparison', () => {
  window.localStorage.setItem(
    'instance:alice:views',
    JSON.stringify([
      viewFromSearch('My comparison', 'evaluation', {
        report: 'candidate', baseline: 'baseline', metrics: 'accuracy', suite: 'suite',
      }),
    ]),
  );
  const onRestore = vi.fn();
  const props = { screen: 'evaluation' as const, search: {}, onRestore };
  const ui = render(<LocalViews {...props} />);
  fireEvent.click(screen.getByRole('button', { name: 'My comparison' }));
  expect(onRestore).toHaveBeenCalledWith(expect.objectContaining({
    report: 'candidate', baseline: 'baseline', metrics: 'accuracy', suite: 'suite',
  }));
  identity.scope = 'instance:bob';
  ui.rerender(<LocalViews {...props} />);
  expect(screen.queryByRole('button', { name: 'My comparison' })).toBeNull();
  identity.scope = undefined;
  ui.rerender(<LocalViews {...props} />);
  fireEvent.change(screen.getByLabelText('View name'), { target: { value: 'Cannot save' } });
  expect((screen.getByRole('button', { name: 'Save view' }) as HTMLButtonElement).disabled).toBe(true);
  identity.scope = 'instance:alice';
  ui.rerender(<LocalViews {...props} />);
  expect(screen.getByRole('button', { name: 'My comparison' })).toBeTruthy();
});

it('reports denied storage without breaking the screen or claiming a save', () => {
  vi.stubGlobal('localStorage', {
    getItem: () => { throw new DOMException('denied', 'SecurityError'); },
  });
  render(<LocalViews screen="training" search={{ runs: ['run'] }} onRestore={vi.fn()} />);
  expect(screen.getByRole('status').textContent).toContain('cannot be read');
  fireEvent.change(screen.getByLabelText('View name'), { target: { value: 'Example' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save view' }));
  expect(screen.getByRole('status').textContent).toContain('Could not save');
});
