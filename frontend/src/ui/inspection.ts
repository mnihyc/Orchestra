import { useRef } from 'react';
/** Portalled inspection cards remain inside the logical focus boundary of the choice that opened them. */
export function isInspectionTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('[data-inspection-card]') !== null;
}

/** Dismissing an overlay can reveal its trigger beneath a stationary pointer. A real pointer leave
 * permits hover again; an explicit click or keyboard focus remains the owning control's decision. */
export function useCardPointerReopen() {
  const allowed = useRef(true);
  return { allowed, dismiss: () => { allowed.current = false; }, leave: () => { allowed.current = true; } };
}
