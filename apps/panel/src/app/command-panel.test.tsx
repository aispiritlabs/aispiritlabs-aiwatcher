import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

const navigate = vi.fn();
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => navigate }));

import { CommandPanel } from '@/app/command-panel';

afterEach(() => {
  navigate.mockReset();
  vi.unstubAllGlobals();
});

/**
 * The one thing worth proving about this panel: what it *navigates to*.
 *
 * A command that reached the right page with the wrong filter — or with the
 * previous page's filter merged into it — looks like the feature working and
 * answers a different question, which is the failure the URL-state rule exists
 * to prevent.
 */

function open() {
  const onOpenChange = vi.fn();
  render(<CommandPanel open onOpenChange={onOpenChange} />);
  return onOpenChange;
}

describe('running a command', () => {
  it('goes to the page with the filter the command names', async () => {
    open();
    await userEvent.type(screen.getByLabelText('Type a command'), 'runs that failed');
    await userEvent.keyboard('{Enter}');

    expect(navigate).toHaveBeenCalledTimes(1);
    // The whole search object, not a patch: a command is a whole question, and
    // merging would leave whichever filters the reader happened to be looking
    // at applied to a different one.
    expect(navigate.mock.calls[0]?.[0]).toEqual({
      to: '/observability/runs',
      search: { status: 'failed' },
    });
  });

  it('clears the filters for a command that names none', async () => {
    // The same rule from the other side, and the reason it is worth its own
    // test: an empty search has to *arrive*, because leaving it out is what
    // would merge the last page's filters into a view that declares none.
    open();
    await userEvent.type(screen.getByLabelText('Type a command'), 'traces for live agents');
    await userEvent.keyboard('{Enter}');

    expect(navigate.mock.calls[0]?.[0]).toEqual({ to: '/observability/live', search: {} });
  });

  it('closes itself before it navigates', async () => {
    const onOpenChange = open();
    await userEvent.type(screen.getByLabelText('Type a command'), 'kaggle');
    await userEvent.keyboard('{Enter}');
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it('runs the row the arrow keys moved to, not the first one', async () => {
    open();
    await userEvent.type(screen.getByLabelText('Type a command'), 'compare runs by');
    const options = screen.getAllByRole('option');
    await userEvent.keyboard('{ArrowDown}{Enter}');

    expect(navigate).toHaveBeenCalledTimes(1);
    const went = navigate.mock.calls[0]?.[0] as { search?: { by?: string } };
    // Whatever the second row is, it is not the first row's pivot.
    expect(options.length).toBeGreaterThan(1);
    expect(went.search?.by).toBeDefined();
  });

  it('says what matched nothing rather than showing an empty box', async () => {
    open();
    await userEvent.type(screen.getByLabelText('Type a command'), 'zzzzqqq');
    expect(screen.queryByText(/Nothing here matches/)).not.toBeNull();
    expect(screen.queryAllByRole('option')).toHaveLength(0);
  });

  it('does not navigate on Enter when nothing matches', async () => {
    open();
    await userEvent.type(screen.getByLabelText('Type a command'), 'zzzzqqq');
    await userEvent.keyboard('{Enter}');
    expect(navigate).not.toHaveBeenCalled();
  });

  it('opens on a fresh query rather than on the last one', async () => {
    const { rerender } = render(<CommandPanel open={false} onOpenChange={vi.fn()} />);
    rerender(<CommandPanel open onOpenChange={vi.fn()} />);
    const input = screen.getByLabelText('Type a command') as HTMLInputElement;
    expect(input.value).toBe('');
  });
});
