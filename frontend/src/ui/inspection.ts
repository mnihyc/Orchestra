import { type PointerEvent as TriggerPointerEvent, useCallback, useEffect, useRef } from 'react';
/** Portalled inspection cards remain inside the logical focus boundary of the choice that opened them. */
export function isInspectionTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('[data-inspection-card]') !== null;
}

/** How long a pointer rests on a trigger, after it moved there, before its card opens: the usual hover card delay. */
export const CARD_OPEN_DELAY_MS = 700;

/**
 * When a card opens under the pointer, and when it does not.
 *
 * A card opens because a person pointed at its trigger: the pointer moved onto it, or moved on it and rested. A trigger that
 * arrives under a pointer that did not move (a dialog or menu that closes, a list that scrolls or reflows, a bar that opens) is
 * announced to the page as an entering, and opens nothing; the pointer's next move on it does. Dismissing a card can also reveal
 * its trigger beneath a stationary pointer: a real leave permits hover again, and an explicit click or keyboard focus remains the
 * owning control's decision. `open` is what a card does when the pointer has pointed at it long enough.
 */
export function useCardPointerReopen(open?: () => void) {
  const allowed = useRef(true);
  const waiting = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const asked = useRef(open);
  useEffect(() => { asked.current = open; });
  const cancel = useCallback(() => { clearTimeout(timer.current); timer.current = undefined; }, []);
  useEffect(() => cancel, [cancel]);
  /** The pointer of a person, on the trigger itself and not on a card or a child that is drawn elsewhere. */
  const onTrigger = (event: TriggerPointerEvent) => event.pointerType !== 'touch' && event.target instanceof Node && event.currentTarget.contains(event.target);
  return {
    allowed,
    dismiss: () => { allowed.current = false; waiting.current = false; cancel(); },
    leave: () => { allowed.current = true; waiting.current = false; cancel(); },
    /** The pointer entered the trigger. The card's own primitive opens on entering, so it is told not to. */
    enter: (event: TriggerPointerEvent) => {
      event.preventDefault();
      if (allowed.current && onTrigger(event)) waiting.current = true;
    },
    /** The pointer moved on the trigger: pointed at, so the card opens after the usual rest. */
    move: (event: TriggerPointerEvent) => {
      if (!waiting.current || !allowed.current || !onTrigger(event)) return;
      waiting.current = false;
      cancel();
      timer.current = setTimeout(() => { timer.current = undefined; asked.current?.(); }, CARD_OPEN_DELAY_MS);
    },
  };
}
