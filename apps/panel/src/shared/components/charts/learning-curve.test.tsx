import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { LearningCurve, seriesStyle } from './learning-curve';

afterEach(() => vi.unstubAllGlobals());

it('keeps the surviving colour and dash when an earlier series is removed', async () => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
    },
  );
  const first = { key: 'a', label: 'A', points: [[0, 1]] as [number, number][] };
  const second = {
    key: 'b',
    label: 'B',
    points: [
      [0, 0.9],
      [2, 0.7],
    ] as [number, number][],
    missing: [1],
  };
  const view = render(<LearningCurve series={[first, second]} />);
  const initial = view.container.querySelectorAll('path')[1]!;
  const colour = initial.getAttribute('stroke');
  const dash = initial.getAttribute('stroke-dasharray');
  view.rerender(<LearningCurve series={[second]} />);
  const path = view.container.querySelector('path')!;
  expect(path.getAttribute('stroke')).toBe(colour);
  expect(path.getAttribute('stroke-dasharray')).toBe(dash);
  expect(colour).toBe(seriesStyle('b').color);
  expect(path.getAttribute('d')?.match(/M/g)).toHaveLength(2);
  await userEvent.click(screen.getByText('Recorded values'));
  expect(screen.getByText('0.9')).toBeTruthy();
  expect(screen.getByText('—')).toBeTruthy();
});
