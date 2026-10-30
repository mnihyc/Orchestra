/** Whether the left bar shows what is archived: one choice for each org on this device, shared by every menu that offers it.
 *
 * Archived projects, topics and threads are hidden from the tree, so that it shows the work that goes on.
 * "Show archived" in the menu of an org or a project brings them back into view, dimmed, where they can be
 * opened and unarchived. The choice is the device's and the person's: it is not asked of the server.
 */
import { useMemo } from 'react';
import { useStore } from 'zustand';
import { create } from 'zustand';

import { client } from '@/api/client';

const storageKey = (userId: string, orgId: string) => `orch.${userId}.archived.${orgId}`;

function read(userId: string, orgId: string): boolean {
  try {
    return localStorage.getItem(storageKey(userId, orgId)) === '1';
  } catch {
    return false;
  }
}

interface Choices {
  /** What was chosen in this page, by `userId:orgId`; what is not here is what the device remembers. */
  made: ReadonlyMap<string, boolean>;
}

const useChoices = create<Choices>(() => ({ made: new Map() }));

/**
 * Whether the archived things of a root org are shown, and the way to change it. The same object comes back
 * for the same answer, so that a tree of thousands of rows that holds it is not drawn again for nothing.
 */
export function useShowArchived(orgId: string | null): { value: boolean; toggle: () => void } {
  const userId = useStore(client.session, (state) => state.userId);
  const made = useChoices((state) => state.made);
  const key = userId === null || orgId === null ? null : `${userId}:${orgId}`;
  const value = userId === null || orgId === null || key === null ? false : (made.get(key) ?? read(userId, orgId));
  return useMemo(
    () => ({
      value,
      toggle: () => {
        if (userId === null || orgId === null || key === null) return;
        try {
          localStorage.setItem(storageKey(userId, orgId), value ? '0' : '1');
        } catch {
          // A device that keeps nothing still shows the choice until the page is closed.
        }
        useChoices.setState((state) => ({ made: new Map(state.made).set(key, !value) }));
      },
    }),
    [value, key, userId, orgId],
  );
}
