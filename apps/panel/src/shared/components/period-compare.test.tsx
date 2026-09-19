import { render } from '@testing-library/react';
import { expect, it } from 'vitest';

import { AgainstPeriod, endOfPeriodBefore, periodLabel } from './period-compare';

it('ends the period before one second earlier than this one begins', () => {
  // Both ends of a window are inclusive, so a run that started exactly at the
  // boundary would otherwise be counted in both halves of the comparison.
  expect(endOfPeriodBefore('2026-09-14T10:00:00Z')).toBe(
    Date.parse('2026-09-14T10:00:00Z') / 1000 - 1,
  );
});

it('names no boundary it cannot read', () => {
  // A caller with no instant asks no second question rather than a vague one.
  expect(endOfPeriodBefore('not a date')).toBeUndefined();
  expect(periodLabel('not a date', 'nor this')).toBe('the previous period');
});

/** Each call renders into a container of its own: two in one test is two answers. */
function change(now: number | null, before: number | null, points = false) {
  const { container } = render(
    <AgainstPeriod now={now} before={before} format={(value) => String(value)} points={points} />,
    { container: document.body.appendChild(document.createElement('div')) },
  );
  return container.textContent;
}

it('reads an absent baseline as unknown rather than as nought', () => {
  // The cost rule: a period whose calls reported no price has an unknown cost,
  // and a percentage against nothing would be an invention.
  expect(change(4, null)).toBe('nothing reported before');
});

it('gives no percentage against a baseline of zero', () => {
  // Nothing is not a denominator. The figure it rose from is still worth
  // saying, and that is all this can honestly say.
  expect(change(12, 0)).toBe('was 0');
});

it('says a change too small to round rather than calling it none', () => {
  expect(change(1001, 1000)).toBe('was 1000 · +under 1%');
  expect(change(1000, 1000)).toBe('was 1000 · unchanged');
});

it('moves a rate by points, because a per cent of a per cent is a third number', () => {
  // 50% to 75% is twenty-five points and fifty per cent, and only the first is
  // the sentence anybody means by "the success rate went up".
  expect(change(0.75, 0.5, true)).toBe('was 0.5 · +25 pts');
});

it('signs a fall with a minus rather than a hyphen', () => {
  expect(change(50, 100)).toBe('was 100 · −50%');
});
