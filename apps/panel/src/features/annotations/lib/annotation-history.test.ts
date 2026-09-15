import { act, renderHook } from '@testing-library/react';
import { expect, it } from 'vitest';
import type { Annotation } from '@/api/generated/types.gen';
import { useAnnotationHistory } from './annotation-history';

const shape = (x: number): Annotation => ({
  id: 'point-1',
  class: 'point',
  geometry: { kind: 'point', at: [x, 2] },
});

it('undoes a whole drag and redoes it, then discards redo after a new edit', () => {
  const { result } = renderHook(useAnnotationHistory);
  act(() => result.current.load('image-1', [shape(1)]));
  act(() => result.current.begin());
  for (const x of [2, 3, 4]) act(() => result.current.change([shape(x)]));
  act(() => result.current.end());
  expect(result.current.past).toHaveLength(1);
  act(() => result.current.undo());
  expect(result.current.present).toEqual([shape(1)]);
  expect(result.current.dirty).toBe(false);
  act(() => result.current.redo());
  expect(result.current.present).toEqual([shape(4)]);
  act(() => result.current.undo());
  act(() => result.current.change([shape(7)]));
  expect(result.current.canRedo).toBe(false);
});

it('keeps history across save and ignores background refetches, but resets for a different image', () => {
  const { result } = renderHook(useAnnotationHistory);
  act(() => result.current.load('image-1', []));
  act(() => result.current.change([{ ...shape(2), attributes: {}, links: {} }]));
  act(() => result.current.markSaved('image-1:saved', [shape(2)]));
  expect(result.current.dirty).toBe(false);
  act(() => result.current.load('image-1:saved', [shape(9)]));
  expect(result.current.present[0]?.geometry).toEqual(shape(2).geometry);
  act(() => result.current.undo());
  expect(result.current.dirty).toBe(true);
  act(() => result.current.redo());
  expect(result.current.dirty).toBe(false);
  act(() => result.current.load('image-2', [shape(8)]));
  expect(result.current.canUndo).toBe(false);
  expect(result.current.canRedo).toBe(false);
  expect(result.current.present).toEqual([shape(8)]);
});

it('bounds retained history and ignores unchanged edits and pointer clicks', () => {
  const { result } = renderHook(useAnnotationHistory);
  act(() => result.current.load('image-1', [shape(0)]));
  act(() => {
    result.current.begin();
    result.current.change([shape(0)]);
    result.current.end();
  });
  expect(result.current.canUndo).toBe(false);
  for (let i = 1; i <= 105; i++) act(() => result.current.change([shape(i)]));
  expect(result.current.past).toHaveLength(100);
  for (let i = 0; i < 100; i++) act(() => result.current.undo());
  expect(result.current.present).toEqual([shape(5)]);
});
