/** Where a dropdown's popup stands, and how tall it may be, for the content it has now (used by Combobox).
 *
 * The popup's own placement decides once, with the content it has when it mounts, and holds its height to the room that
 * side had then; a list that arrives after the popup is open (every server-backed picker) then fills a room that was
 * measured for something else. This measures the content as it is, again whenever its size, the window or the page
 * under it changes, and gives the rule in `dropdownRoom` (src/ui/floating.ts) the side and the height to hand back.
 */
import { type RefObject, useLayoutEffect, useRef, useState } from 'react';

import { dropdownRoom } from '@/ui/floating';

export interface DropdownPlace {
  side: 'top' | 'bottom';
  /** The room on that side, which the popup's own height may not pass; unset until it has been measured. */
  height: number | undefined;
}

const START: DropdownPlace = { side: 'bottom', height: undefined };

/** What the screen shows of the page, as Floating UI measures it: the visual viewport when the browser has one. */
function viewportBox(): { top: number; bottom: number } {
  const view = window.visualViewport as VisualViewport | null | undefined;
  return view ? { top: view.offsetTop, bottom: view.offsetTop + view.height } : { top: 0, bottom: document.documentElement.clientHeight };
}

/** The popup's whole height if nothing held it, and the least of it worth showing: its own parts and `rows` of the list. */
function measure(popup: HTMLElement, hasFooter: boolean, rows: number): { natural: number; least: number } | null {
  const list = popup.querySelector<HTMLElement>('[data-combobox-options]');
  if (list === null) return null;
  // With a footer the list scrolls inside the popup; without one the popup scrolls its whole content.
  const scroller = hasFooter ? list : popup;
  const natural = Math.ceil(popup.offsetHeight - scroller.clientHeight + scroller.scrollHeight);
  if (!(natural > 0)) return null;
  const count = list.querySelectorAll('[role="option"]').length;
  const whole = hasFooter ? list.scrollHeight : list.offsetHeight;
  // A popup with no row to show (loading, nothing found) is shown whole. A row is the list's mean: the least it asks
  // must not change with which row the pointer or the keyboard is on, or the popup would change sides under them.
  return { natural, least: count === 0 ? natural : Math.min(natural, Math.ceil(natural - whole + Math.min(count, rows) * (whole / count))) };
}

export function useDropdownRoom({ open, covered, anchor, popup, version, watch, hasFooter, rows, gap }: {
  open: boolean;
  /** Hidden behind a prompt: there is nothing to measure. */
  covered: boolean;
  anchor: RefObject<HTMLElement | null>;
  popup: RefObject<HTMLElement | null>;
  /** Changes when the popup's node is replaced. */
  version: number;
  /** Changes when the list is replaced. */
  watch: unknown;
  hasFooter: boolean;
  /** How many rows of the list the popup promises before it looks for room on the other side. */
  rows: number;
  /** The gap the popup keeps from its field. */
  gap: number;
}): DropdownPlace {
  const [place, setPlace] = useState<DropdownPlace>(START);
  // The side this popup was last given, once it has been given one: the first decision is made with nothing to keep.
  // Floating UI's own `data-side` is not asked: it trails a decision by a frame and would undo it.
  const stood = useRef<'above' | 'below' | null>(null);

  // A popup that is put away, or hidden, is measured afresh when it comes back.
  const shown = open && !covered;
  const [wasShown, setWasShown] = useState(shown);
  if (wasShown !== shown) { setWasShown(shown); if (!shown) setPlace(START); }

  useLayoutEffect(() => {
    if (!shown) { stood.current = null; return; }
    const frame = popup.current, field = anchor.current;
    if (frame === null || field === null) return;
    let live = true, pending = 0;
    const decide = () => {
      pending = 0;
      if (!live) return;
      const size = measure(frame, hasFooter, rows);
      const viewport = viewportBox();
      if (size === null || !(viewport.bottom > viewport.top)) return;
      const room = dropdownRoom({ anchor: field.getBoundingClientRect(), viewport, ...size, ...(stood.current === null ? {} : { now: stood.current }), gap });
      stood.current = room.above ? 'above' : 'below';
      const height = Math.floor(room.height);
      const next: DropdownPlace = { side: room.above ? 'top' : 'bottom', height: height > 0 ? height : undefined };
      setPlace((was) => was.side === next.side && was.height === next.height ? was : next);
    };
    const later = () => { if (live && pending === 0) pending = requestAnimationFrame(decide); };
    decide();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(later);
    if (observer !== null) {
      const list = frame.querySelector<HTMLElement>('[data-combobox-options]');
      for (const node of [field, frame, list, frame.querySelector<HTMLElement>('[data-combobox-footer]'), ...Array.from(list?.children ?? [])]) if (node !== null) observer.observe(node);
    }
    // The page moving under the popup (a dialog scrolled, a window resized) changes the room on each side of the field.
    const moved = (event: Event) => { if (!(event.target instanceof Node) || !frame.contains(event.target)) later(); };
    window.addEventListener('resize', later);
    window.visualViewport?.addEventListener('resize', later);
    window.visualViewport?.addEventListener('scroll', later);
    document.addEventListener('scroll', moved, { capture: true, passive: true });
    return () => {
      live = false;
      cancelAnimationFrame(pending);
      observer?.disconnect();
      window.removeEventListener('resize', later);
      window.visualViewport?.removeEventListener('resize', later);
      window.visualViewport?.removeEventListener('scroll', later);
      document.removeEventListener('scroll', moved, { capture: true });
    };
  }, [shown, version, watch, hasFooter, rows, gap, anchor, popup]);

  return place;
}
