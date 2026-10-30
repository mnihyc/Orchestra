/** The hints that are on the screen in view, so that the help can list them: a hint says that it is here while it is drawn, and that it is gone when it goes. */
import { useSyncExternalStore } from 'react';

import type { HintId } from '@/features/help/hints';

/** How many of each hint are drawn: the same hint may stand beside several fields at once. */
const counts = new Map<HintId, number>();
const listeners = new Set<() => void>();
let snapshot: readonly HintId[] = [];

function changed(): void {
  snapshot = [...counts.keys()];
  for (const listener of listeners) listener();
}

/** Says that a hint is drawn. What it returns says that it is not any more. */
export function noteHint(id: HintId): () => void {
  counts.set(id, (counts.get(id) ?? 0) + 1);
  changed();
  return () => {
    const left = (counts.get(id) ?? 1) - 1;
    if (left <= 0) counts.delete(id);
    else counts.set(id, left);
    changed();
  };
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The hints on the screen, in the order they were first drawn. */
export function useMountedHints(): readonly HintId[] {
  return useSyncExternalStore(subscribe, () => snapshot);
}
