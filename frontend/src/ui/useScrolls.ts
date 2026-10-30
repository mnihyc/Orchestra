/** Whether a box holds more than it shows, kept current as the box and what is in it change size.
 *
 * A box that scrolls has to take the keyboard, or a person without a pointer cannot read what lies below its edge; a
 * box that does not scroll takes none, since a stop that leads nowhere is one stop too many. Give `ref` to the box and
 * spread `scrollProps` on it: it is a tab stop while it scrolls, and is not while it does not.
 *
 * `attach` is the callback to give as the box's `ref`; it is no ref object, and is called by a name of its own for that reason.
 */
import { useEffect, useState } from 'react';

export function useScrolls(): { attach: (element: HTMLElement | null) => void; scrolls: boolean; scrollProps: { tabIndex?: 0 } } {
  const [box, setBox] = useState<HTMLElement | null>(null);
  const [scrolls, setScrolls] = useState(false);
  useEffect(() => {
    if (box === null || typeof ResizeObserver === 'undefined') return;
    // One pixel of room for the rounding of a box that is as tall as what it holds. An observer tells of its elements as it begins to watch them.
    const measure = () => setScrolls(box.scrollHeight > box.clientHeight + 1);
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    for (const child of Array.from(box.children)) observer.observe(child);
    return () => observer.disconnect();
  }, [box]);
  return { attach: setBox, scrolls, scrollProps: scrolls ? { tabIndex: 0 } : {} };
}
