/** Adds an action to an existing shared control without another element, style or focus stop. */
import { Slot } from 'radix-ui';
import type { ReactNode } from 'react';

export function ActionTrigger({ children, pending = false, onAction }: { children: ReactNode; pending?: boolean; onAction: () => void }) {
  return <Slot.Root aria-disabled={pending || undefined} aria-busy={pending || undefined} onClick={(event) => {
    // The child's own permission/refusal handler runs first and can cancel the action.
    if (event.defaultPrevented) return;
    event.preventDefault();
    if (!pending) onAction();
  }}>{children}</Slot.Root>;
}
