/** A hidden bar opens by its button in the top bar and by its key, and nothing opens it because the pointer crossed an edge, left the window over
 * one, rested on a scroll bar or dragged it. A person who wants hidden bars to peek over the page says so (Settings, Interface); the bar then
 * peeks when the pointer has rested in the strip at its edge, and goes when the pointer leaves it. A peek is not an opening: what is open is not changed.
 *
 * The strip is not a thing in the page: it is where the pointer is, so it covers no scroll bar and no control and takes no press from any. On the right it is the
 * outermost 2 px of the window, so that the body of a scroll bar at the edge, where a hand rests before it takes hold, is outside it; a press cancels the wait, and
 * a hand that holds a button down or scrolls is not waiting for a bar.
 */
import { useEffect, useRef } from 'react';
import type { SideBarId } from './layout';

/** How long the pointer rests in the strip before the bar peeks. A pointer crossing the strip on its way to the window's edge, or to a scroll bar beside it,
 * takes about a tenth of a second at the pace of a hand; half a second is where a rest reads as a wish for the bar (the dock of a desktop shows after as long);
 * 400 ms is a little under that, so that a rest is answered and a pointer on its way elsewhere is not. */
export const PEEK_DWELL_MS = 400;
/** How wide the strip beside the rail is: the width at which that edge stays easy to point at (`--bar-reveal-width`, 12 px). */
export const PEEK_EDGE_PX = 12;
/** How wide the right bar's strip is: the outermost 2 px of the window, the pointer pushed against the edge as a desktop dock is called. The scroll bar of a page
 * that reaches the window's edge is 15 px wide there, so its body, where a hand rests before it takes hold, is outside the strip. */
export const PEEK_RIGHT_EDGE_PX = 2;
/** How long after the last scroll the person is taken to be scrolling still. */
const SCROLLING_MS = 250;
/** What a press at the edge would be for, if it were not the edge: the strip leaves every one of these to its own. */
const CONTROL = 'a[href], button, input, select, textarea, summary, [role="button"], [role="link"], [role="menuitem"], [role="tab"], [role="switch"], [role="checkbox"], [role="radio"], [role="option"], [role="slider"], [tabindex]:not([tabindex="-1"])';

/** Where the pointer was last seen: a peeking bar stays while the pointer is in the strip beside it. */
let seenX: number | null = null;
export const pointerSeen = (): number | null => seenX;
export function noteSeen(x: number | null): void { seenX = x; }

/** The bar whose strip holds the pointer: the right one at the very edge of the window, the left one at the edge of the page beside the rail. */
export function edgeOf(x: number, bars: readonly SideBarId[]): SideBarId | null {
  if (bars.includes('right') && x >= innerWidth - PEEK_RIGHT_EDGE_PX && x < innerWidth) return 'right';
  if (bars.includes('left')) {
    const rail = document.querySelector('[data-navigation-rail]')?.getBoundingClientRect().right ?? 0;
    if (x >= rail && x < rail + PEEK_EDGE_PX) return 'left';
  }
  return null;
}

/** Calls `onPeek` with the bar whose edge the pointer has rested at; `bars` are the hidden ones that can peek. */
export function usePeek({ enabled, bars, onPeek }: { enabled: boolean; bars: readonly SideBarId[]; onPeek: (bar: SideBarId) => void }): void {
  const peek = useRef(onPeek);
  useEffect(() => { peek.current = onPeek; });
  const named = bars.join(' ');
  useEffect(() => {
    if (!enabled || named === '') return;
    const watched = named.split(' ') as SideBarId[];
    let timer: ReturnType<typeof setTimeout> | undefined;
    let waiting: SideBarId | null = null;
    let scrolled = Number.NEGATIVE_INFINITY;
    const cancel = () => { clearTimeout(timer); timer = undefined; waiting = null; };
    const move = (event: PointerEvent) => {
      noteSeen(event.clientX);
      // A hand that holds a button down is dragging, and one that is scrolling is not asking for a bar.
      if (event.pointerType === 'touch' || event.buttons !== 0 || performance.now() - scrolled < SCROLLING_MS) { cancel(); return; }
      const edge = edgeOf(event.clientX, watched);
      if (edge === null) { cancel(); return; }
      const under = document.elementFromPoint(event.clientX, event.clientY);
      if (under === null) { cancel(); return; }
      // The handle that sizes a bar is a separator: at the window's last pixel it is the edge itself, and not a control that the strip covers.
      const control = under.closest(CONTROL);
      if (control !== null && control.getAttribute('role') !== 'separator') { cancel(); return; }
      if (waiting === edge) return;
      cancel();
      waiting = edge;
      timer = setTimeout(() => { const bar = waiting; cancel(); if (bar !== null) peek.current(bar); }, PEEK_DWELL_MS);
    };
    const scrolling = () => { scrolled = performance.now(); cancel(); };
    const left = () => { noteSeen(null); cancel(); };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerdown', cancel, true);
    document.addEventListener('scroll', scrolling, true);
    document.documentElement.addEventListener('pointerleave', left);
    document.documentElement.addEventListener('mouseleave', left);
    window.addEventListener('blur', cancel);
    return () => {
      cancel();
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerdown', cancel, true);
      document.removeEventListener('scroll', scrolling, true);
      document.documentElement.removeEventListener('pointerleave', left);
      document.documentElement.removeEventListener('mouseleave', left);
      window.removeEventListener('blur', cancel);
    };
  }, [enabled, named]);
}
