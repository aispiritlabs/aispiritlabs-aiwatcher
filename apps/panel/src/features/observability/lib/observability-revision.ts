import { useSyncExternalStore } from 'react';

/**
 * A tiny process-local signal shared by the Observability layout and views
 * whose result is not held in React Query (the ad-hoc Query tab).
 */
let revision = 0;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): number {
  return revision;
}

export function publishObservabilityRevision(): void {
  revision += 1;
  for (const listener of listeners) listener();
}

export function useObservabilityRevision(): number {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
