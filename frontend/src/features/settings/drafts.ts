/** The structured values (objects and lists) being edited as a draft, which nothing but their own Save or Discard may write or drop.
 *
 * Each row owns one hold in its workbench instance. Activity may pause the row's effects, but only saving, discarding or removing its view releases that hold.
 */
import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { create } from 'zustand';

import { useWorkbench } from '@/features/workbench/store';
import { useViewSaveGuard } from '@/features/workbench/CloseViews';
import { useViewInstance } from '@/features/workbench/instance';
import { allTabs } from '@/features/workbench/layout';

export interface DraftHold {
  /** Writes the draft; whether it was written. */
  save: () => Promise<boolean>;
  /** Puts the saved value back. */
  discard: () => void;
}

const holds = new Map<symbol, { view: string | undefined; draft: DraftHold }>();
const useHeld = create<{ counts: ReadonlyMap<string | undefined, number> }>(() => ({ counts: new Map() }));
const marked = new Set<string>();

function publish(): void {
  const counts = new Map<string | undefined, number>();
  for (const { view } of holds.values()) counts.set(view, (counts.get(view) ?? 0) + 1);
  useHeld.setState({ counts });
  for (const view of new Set([...marked, ...counts.keys()])) {
    if (view === undefined) continue;
    const dirty = counts.has(view);
    if (dirty) marked.add(view); else marked.delete(view);
    useWorkbench.getState().setUnsaved(view, dirty);
  }
}

function release(id: symbol): void {
  if (holds.delete(id)) publish();
}

/** How many drafts this view holds, including its paused sections and pages. */
export function useUnsavedDrafts(): number {
  const view = useViewInstance()?.id;
  return useHeld((state) => state.counts.get(view) ?? 0);
}

/** Distinct row identities keep two editors of the same setting independent. */
export function useHoldDraft(holding: boolean, draft: DraftHold): void {
  const view = useViewInstance()?.id;
  const [id] = useState(() => Symbol('setting draft'));
  const latest = useRef(draft);
  useLayoutEffect(() => { latest.current = draft; });
  useEffect(() => {
    if (holding) {
      holds.set(id, { view, draft: { save: () => latest.current.save(), discard: () => latest.current.discard() } });
      publish();
    } else release(id);
    return () => {
      // Hiding a kept view or one of its pages pauses effects without ending draft ownership.
      if (view === undefined || !allTabs(useWorkbench.getState().layout).some((tab) => tab.id === view)) release(id);
    };
  }, [id, view, holding]);
  useDraftGuard();
}

/** The workbench's existing loss question acts only on drafts owned by the affected view. */
export function useDraftGuard(): void {
  const view = useViewInstance()?.id;
  const guard = useMemo(() => ({
    save: async () => {
      let saved = true;
      for (const [id, held] of [...holds]) {
        if (held.view !== view) continue;
        if (await held.draft.save()) { if (holds.get(id) === held) release(id); }
        else saved = false;
      }
      return saved;
    },
    discard: () => {
      for (const [id, held] of [...holds]) {
        if (held.view !== view) continue;
        held.draft.discard();
        if (holds.get(id) === held) release(id);
      }
    },
  }), [view]);
  useViewSaveGuard(view, guard);
}

useWorkbench.subscribe((next, before) => {
  if (next.layout === before.layout) return;
  const live = new Set(allTabs(next.layout).map((tab) => tab.id));
  let removed = false;
  for (const [id, held] of holds) if (held.view !== undefined && !live.has(held.view)) { holds.delete(id); removed = true; }
  if (removed) publish();
});

/** What the server said was wrong with a draft, by the member each is about (`thread.min_interval_secs`); the row supplies it to the editor of its value. */
export const DraftFaults = createContext<readonly { path: string; message: string }[]>([]);

/** The server's fault for one member of the draft being edited, to be shown beside that member's field. */
export function useDraftFault(path: string): string | undefined {
  return useContext(DraftFaults).find((fault) => fault.path === path || fault.path === `value.${path}`)?.message;
}

/** Where an editor reports the first thing in its draft that cannot be saved as it stands (one it learned from the server, for a provider or a model), or none. */
export const DraftValidity = createContext<((problem: string | null) => void) | null>(null);

/** Reports the problem of the draft to the row that holds it, and takes it back when the editor goes. */
export function useReportValidity(problem: string | null): void {
  const report = useContext(DraftValidity);
  useEffect(() => {
    report?.(problem);
    return () => report?.(null);
  }, [report, problem]);
}
