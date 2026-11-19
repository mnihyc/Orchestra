/** Portalled inspection cards remain inside the logical focus boundary of the choice that opened them. */
export function isInspectionTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('[data-inspection-card]') !== null;
}
