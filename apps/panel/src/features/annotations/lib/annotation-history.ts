import { useCallback, useReducer } from 'react';

import type { Annotation } from '@/api/generated/types.gen';
import { sameAnnotations } from './annotations';

const LIMIT = 100;
type Drawing = Annotation[];
type State = {
  context: string;
  past: Drawing[];
  present: Drawing;
  future: Drawing[];
  saved: Drawing;
  gesture: Drawing | null;
};
type Action =
  | { type: 'load' | 'saved'; context: string; annotations: Drawing }
  | { type: 'change'; annotations: Drawing }
  | { type: 'begin' | 'end' | 'undo' | 'redo' };

function initial(context: string, annotations: Drawing): State {
  return { context, past: [], present: annotations, future: [], saved: annotations, gesture: null };
}

function reduce(state: State, action: Action): State {
  switch (action.type) {
    case 'load':
      // A background refetch must never replace local work or its baseline.
      return state.context === action.context ? state : initial(action.context, action.annotations);
    case 'saved':
      return { ...state, context: action.context, saved: action.annotations };
    case 'change':
      if (sameAnnotations(state.present, action.annotations)) return state;
      return {
        ...state,
        past: state.gesture ? state.past : [...state.past, state.present].slice(-LIMIT),
        present: action.annotations,
        future: [],
      };
    case 'begin':
      return state.gesture ? state : { ...state, gesture: state.present };
    case 'end':
      return {
        ...state,
        past:
          state.gesture && !sameAnnotations(state.gesture, state.present)
            ? [...state.past, state.gesture].slice(-LIMIT)
            : state.past,
        gesture: null,
      };
    case 'undo': {
      const previous = state.past.at(-1);
      if (!previous || state.gesture) return state;
      return {
        ...state,
        past: state.past.slice(0, -1),
        present: previous,
        future: [state.present, ...state.future],
      };
    }
    case 'redo': {
      const next = state.future[0];
      if (!next || state.gesture) return state;
      return {
        ...state,
        past: [...state.past, state.present].slice(-LIMIT),
        present: next,
        future: state.future.slice(1),
      };
    }
  }
}

/** Local history; immutable registry revisions remain explicit Save operations. */
export function useAnnotationHistory() {
  const [state, dispatch] = useReducer(reduce, initial('', []));
  const load = useCallback(
    (context: string, annotations: Drawing) => dispatch({ type: 'load', context, annotations }),
    [],
  );
  const markSaved = useCallback(
    (context: string, annotations: Drawing) => dispatch({ type: 'saved', context, annotations }),
    [],
  );
  const change = useCallback(
    (annotations: Drawing) => dispatch({ type: 'change', annotations }),
    [],
  );
  const begin = useCallback(() => dispatch({ type: 'begin' }), []);
  const end = useCallback(() => dispatch({ type: 'end' }), []);
  const undo = useCallback(() => dispatch({ type: 'undo' }), []);
  const redo = useCallback(() => dispatch({ type: 'redo' }), []);
  return {
    ...state,
    load,
    markSaved,
    change,
    begin,
    end,
    undo,
    redo,
    dirty: !sameAnnotations(state.present, state.saved),
    canUndo: state.past.length > 0 && !state.gesture,
    canRedo: state.future.length > 0 && !state.gesture,
  };
}
