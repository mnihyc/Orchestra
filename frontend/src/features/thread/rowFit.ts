/** One row that never wraps: the acts that do not fit leave it, the last first, and the title keeps its room.
 *
 * The row marks what may leave with `data-fold="<id>"` (in the order they are to stay, first to last) and what
 * always stays with `data-fold-fixed`. What leaves is hidden, never moved, so that its width is still known when the
 * row grows again; the owner of the row offers each of them in its menu.
 */
import { type RefObject, useLayoutEffect, useRef, useState } from 'react';

/** How many of the items, counted from the first, fit in `room` with `gap` between them. */
export function fitCount(room: number, widths: readonly number[], gap: number): number {
  let used = 0;
  for (let at = 0; at < widths.length; at += 1) {
    const next = used + (at === 0 ? 0 : gap) + (widths[at] ?? 0);
    if (next > room) return at;
    used = next;
  }
  return widths.length;
}

const NOTHING: ReadonlySet<string> = new Set();

/** The mark beside a title that a click renames takes this much room, shown or not. */
const RENAME_MARK_PX = 28;

const pixels = (value: string): number => {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : 0;
};

/**
 * The ids that do not fit the row now. `ids` is the order the items stay in, so the last of them leaves first. The room
 * is the row's width less what always stays, less the least the title may be given (its own `min-width`).
 */
export function useRowFit(row: RefObject<HTMLElement | null>, ids: readonly string[]): ReadonlySet<string> {
  const [hidden, setHidden] = useState<ReadonlySet<string>>(NOTHING);
  // What each item measured the last time it was drawn: a hidden item is not drawn.
  const widths = useRef(new Map<string, number>());
  const key = ids.join('|');
  useLayoutEffect(() => {
    const element = row.current;
    if (element === null) return;
    const order = key === '' ? [] : key.split('|');
    const measure = () => {
      const available = element.clientWidth;
      // A renderer without layout (a test) keeps every item.
      if (available <= 0) return;
      const style = getComputedStyle(element);
      const gap = pixels(style.columnGap);
      for (const id of order) {
        const item = element.querySelector<HTMLElement>(`[data-fold="${id}"]`);
        if (item !== null && !item.hidden) {
          const width = item.getBoundingClientRect().width;
          if (width > 0) widths.current.set(id, width);
        }
      }
      let fixed = 0;
      let count = 0;
      for (const item of element.querySelectorAll<HTMLElement>('[data-fold-fixed]')) {
        fixed += item.getBoundingClientRect().width;
        count += 1;
      }
      // The title keeps the room its name needs, up to half of the row, and never less than its own least width.
      const title = element.querySelector<HTMLElement>('[data-fold-title]');
      const heading = title === null ? null : title.querySelector('h1');
      const natural = heading === null ? 0 : heading.scrollWidth + RENAME_MARK_PX;
      const least = title === null ? 0 : Math.max(pixels(getComputedStyle(title).minWidth), Math.min(natural, available / 2));
      // The row's own padding is outside what `clientWidth` offers to children.
      const room = available - pixels(style.paddingLeft) - pixels(style.paddingRight) - fixed - least - (count + 1) * gap;
      const fits = fitCount(room, order.map((id) => widths.current.get(id) ?? 0), gap);
      const next = new Set(order.slice(fits));
      setHidden((held) => held.size === next.size && [...next].every((id) => held.has(id)) ? held : next);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    for (const item of element.querySelectorAll('[data-fold], [data-fold-fixed]')) observer.observe(item);
    return () => observer.disconnect();
  }, [row, key]);
  return hidden;
}
