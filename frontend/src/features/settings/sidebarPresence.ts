/** An explicitly selected Resources view can supply its own kind links; page sections never occupy global Navigation. */
import { useId, useLayoutEffect } from 'react';
import { create } from 'zustand';

import { useViewShown } from '@/features/workbench/viewOpening';

type Scope = 'resources';
const useSidebars = create<{ slots: ReadonlyMap<string, Scope> }>(() => ({ slots: new Map() }));

export function useSettingsSidebarPresence(scope: Scope | undefined): boolean {
  return useSidebars((state) => scope !== undefined && Array.from(state.slots.values()).includes(scope));
}

export function useSettingsSidebarSlot(scope: Scope): void {
  const id = useId();
  const shown = useViewShown();
  useLayoutEffect(() => {
    if (!shown) return;
    useSidebars.setState((state) => ({ slots: new Map(state.slots).set(id, scope) }));
    return () => useSidebars.setState((state) => { const slots = new Map(state.slots); slots.delete(id); return { slots }; });
  }, [id, scope, shown]);
}
