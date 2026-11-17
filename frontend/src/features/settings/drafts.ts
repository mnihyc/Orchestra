/** The structured values (objects and lists) being edited as a draft, which nothing but their own Save or Discard may write or drop.
 *
 * Each open draft holds itself here while it differs from what is saved. The workbench reads the count to mark its view as having unsaved changes, so that
 * closing the view, reloading the page or changing section asks once; the same hold lets that one question save or discard every draft.
 */
import { createContext, useContext, useEffect } from 'react';
import { create } from 'zustand';

import { useWorkbench } from '@/features/workbench/store';
import { useViewSaveGuard } from '@/features/workbench/CloseViews';
import { useViewInstance } from '@/features/workbench/instance';

export interface DraftHold {
  /** Writes the draft; whether it was written. */
  save: () => Promise<boolean>;
  /** Puts the saved value back. */
  discard: () => void;
}

const holds = new Map<string, DraftHold>();
const useHeld = create<{ count: number }>(() => ({ count: 0 }));

/** How many drafts are open and unsaved. */
export function useUnsavedDrafts(): number {
  return useHeld((state) => state.count);
}

/** Holds one draft until it is released; the returned function lets go of it. */
export function holdDraft(id: string, hold: DraftHold): () => void {
  holds.set(id, hold);
  useHeld.setState({ count: holds.size });
  return () => {
    if (holds.get(id) !== hold) return;
    holds.delete(id);
    useHeld.setState({ count: holds.size });
  };
}

async function saveAll(): Promise<boolean> {
  const results = await Promise.all([...holds.values()].map((hold) => hold.save()));
  return results.every(Boolean);
}

function discardAll(): void {
  for (const hold of [...holds.values()]) hold.discard();
}

/** The one question every open draft shares. */
const GUARD = { save: saveAll, discard: discardAll };

/** Marks the view this editor stands in as having unsaved changes while any draft is held, and answers the question closing it asks. */
export function useDraftGuard(): void {
  const instance = useViewInstance()?.id;
  const unsaved = useUnsavedDrafts() > 0;
  useEffect(() => {
    if (instance === undefined) return;
    useWorkbench.getState().setUnsaved(instance, unsaved);
    return () => useWorkbench.getState().setUnsaved(instance, false);
  }, [instance, unsaved]);
  useViewSaveGuard(instance, GUARD);
}

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
