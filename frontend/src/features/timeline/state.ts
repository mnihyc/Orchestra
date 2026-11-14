/** What the reader opened or closed by hand in a conversation, kept for as long as the screen is open.
 *
 * How much of the agent's work is open by itself follows the way the thread is read and whether its turn is
 * still at it; what they then open or close by hand holds while the screen is open, and is forgotten with it: a
 * step, the group of work that holds steps, and the raw call under a step.
 */
import { create } from 'zustand';
import { client } from '@/api/client';
import { defaultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useMemo } from 'react';
import { keepPresentation, readPresentation } from '@/features/workbench/presentation';

interface ThreadUi {
  /** Steps and groups of work the reader opened (`true`) or closed (`false`), against what they are drawn as by themselves. */
  steps: ReadonlyMap<string, boolean>;
  /** Steps whose raw call is shown in place of their display. */
  raw: ReadonlySet<string>;
  /** The newest explicit request to reveal an aside, consumed once its start marker is held. */
  sideFocus?: { id: string; request: number };
}

const NOTHING: ThreadUi = { steps: new Map(), raw: new Set() };

function keptUi(target: ThreadReadTarget): ThreadUi {
  const read = typeof target === 'string' ? defaultThreadRead(target) : target;
  const value = read.key.kind === 'result' ? readPresentation(read.key.owner, `timeline:${read.threadId}`) : undefined;
  if (typeof value !== 'object' || value === null || !('version' in value) || value.version !== 1) return NOTHING;
  const strings = (value: unknown): string[] => Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
  const steps = 'steps' in value && Array.isArray(value.steps) ? value.steps.filter((entry): entry is [string, boolean] => Array.isArray(entry) && entry.length === 2 && typeof entry[0] === 'string' && typeof entry[1] === 'boolean') : [];
  return { steps: new Map(steps), raw: new Set(strings('raw' in value ? value.raw : null)) };
}

interface TimelineUi {
  byThread: ReadonlyMap<string, ThreadUi>;
}

export const useTimelineUi = create<TimelineUi>(() => ({ byThread: new Map() }));

function localKey(target: ThreadReadTarget): string {
  const read = typeof target === 'string' ? defaultThreadRead(target) : target;
  return read.key.kind === 'thread' ? read.threadId : JSON.stringify([read.key.kind, read.threadId, read.key.owner]);
}

client.onSession((event) => {
  if (event.type === 'signed_in' || event.type === 'signed_out') useTimelineUi.setState({ byThread: new Map() });
});

function change(target: ThreadReadTarget, make: (ui: ThreadUi) => ThreadUi): void {
  const threadId = localKey(target);
  useTimelineUi.setState((state) => {
    const byThread = new Map(state.byThread);
    const next = make(state.byThread.get(threadId) ?? keptUi(target));
    byThread.set(threadId, next);
    const read = typeof target === 'string' ? defaultThreadRead(target) : target;
    if (read.key.kind === 'result') keepPresentation(read.key.owner, `timeline:${read.threadId}`, { version: 1, steps: [...next.steps], raw: [...next.raw] });
    return { byThread };
  });
}

const toggled = (set: ReadonlySet<string>, id: string): Set<string> => {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
};

/** Aside openings share the existing per-thread expansion state without colliding with tool item ids. */
export const sideLaneKey = (sideId: string): string => `side:${sideId}`;

export const timelineUi = {
  /** An accepted aside opens inline once its durable start marker arrives. */
  openSide(threadId: ThreadReadTarget, sideId: string): void {
    change(threadId, (ui) => ({ ...ui, steps: new Map(ui.steps).set(sideLaneKey(sideId), true), sideFocus: { id: sideId, request: (ui.sideFocus?.request ?? 0) + 1 } }));
  },
  /** A running child's header chip opens its own step before the timeline scrolls there, which opens the group that holds it. */
  revealStep(threadId: ThreadReadTarget, itemId: string): void {
    change(threadId, (ui) => {
      const raw = new Set(ui.raw);
      raw.delete(itemId);
      return { ...ui, steps: new Map(ui.steps).set(itemId, true), raw };
    });
  },
  /** Opens or closes a step, to the opposite of how it is shown now. */
  setStep(threadId: ThreadReadTarget, itemId: string, open: boolean): void {
    change(threadId, (ui) => ({ ...ui, steps: new Map(ui.steps).set(itemId, open) }));
  },
  toggleRaw(threadId: ThreadReadTarget, itemId: string): void {
    change(threadId, (ui) => ({ ...ui, raw: toggled(ui.raw, itemId) }));
  },
  forget(target: ThreadReadTarget): void {
    const threadId = localKey(target);
    useTimelineUi.setState((state) => {
      if (!state.byThread.has(threadId)) return state;
      const byThread = new Map(state.byThread);
      byThread.delete(threadId);
      return { byThread };
    });
  },
};

export function useThreadUi(threadId: string): ThreadUi {
  const read = useThreadReadRef(threadId);
  const restored = useMemo(() => keptUi(read), [read]);
  return useTimelineUi((state) => state.byThread.get(localKey(read)) ?? restored);
}
