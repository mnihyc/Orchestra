/** After Send the sent message is brought to the top of the view and the reply fills the room below it.
 *
 * The conversation is as long as what was said, so the message that was just sent cannot stand at the top of the view
 * until there is room under it: a spacer at the end of the scrolling part gives that room, and gives back what the
 * reply takes of it, until the reply is as tall as the view. It is set by hand, not by state: the reply grows many times
 * a second, and the end of the conversation must be where the list says it is the moment it is asked.
 */
import { useCallback, useEffect, useMemo, useRef } from 'react';

/** The room that has to follow the conversation so that a place `top` pixels down can stand at the top of a view `viewHeight` high. */
export function roomFor(top: number, viewHeight: number, contentEnd: number): number {
  return Math.max(0, top + viewHeight - contentEnd);
}

/** Where `element` begins, in the scroll coordinates of `viewport`. */
export function topWithin(viewport: HTMLElement, element: HTMLElement): number {
  return viewport.scrollTop + element.getBoundingClientRect().top - viewport.getBoundingClientRect().top;
}

export interface ReplyRoom {
  /** Puts `element` at the top of `scroller`, the view it stands in, and keeps room for it. */
  hold: (scroller: HTMLElement, element: HTMLElement) => void;
  /** Gives the room back. */
  free: () => void;
}

/** Gives the spacer the height that lets a place `top` pixels down stand at the top of `viewport`. The spacer's own height is not part of what it has to make up. */
function fitRoom(viewport: HTMLElement, spacer: HTMLElement, top: number): void {
  spacer.style.height = `${roomFor(top, viewport.clientHeight, viewport.scrollHeight - spacer.offsetHeight)}px`;
}

/** Takes the room away. */
function giveBack(spacer: HTMLElement): void {
  spacer.style.height = '0px';
}

/** `spacer` is the element drawn last in the scrolling part of `viewport` (the owner holds it in state, from its `ref`). */
export function useReplyRoom(viewport: HTMLElement | null, spacer: HTMLElement | null, held: boolean): ReplyRoom {
  const top = useRef<number | null>(null);
  const hold = useCallback((scroller: HTMLElement, element: HTMLElement) => {
    const at = topWithin(scroller, element);
    top.current = at;
    if (spacer !== null) fitRoom(scroller, spacer, at);
    scroller.scrollTop = at;
  }, [spacer]);
  const free = useCallback(() => {
    top.current = null;
    if (spacer !== null) giveBack(spacer);
  }, [spacer]);
  // While the place is held, the room follows what the reply does to the length of the conversation and to the view.
  useEffect(() => {
    if (!held) {
      top.current = null;
      if (spacer !== null) giveBack(spacer);
      return;
    }
    if (viewport === null || spacer === null || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (top.current !== null) fitRoom(viewport, spacer, top.current);
    });
    observer.observe(viewport);
    for (const child of viewport.children) if (child !== spacer) observer.observe(child);
    return () => observer.disconnect();
  }, [held, viewport, spacer]);
  return useMemo(() => ({ hold, free }), [hold, free]);
}
