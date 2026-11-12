/** Where a floating layer above or below what opened it stands, and how tall it may be, for the content it has now.
 *
 * Radix places a layer once, with the content it has when it mounts, and `floating-layer` then holds its height to the
 * room that side had then; a list or a card whose content arrives after the layer is open (anything read from the server)
 * grows into a room that was measured for something else, and Radix never flips it because the held content "fits".
 * This measures the content as it is, again whenever its size, the window or the page under it changes, and gives the
 * rule in `layerRoom` (src/ui/floating.ts) the side and the height to hand back. A layer beside what opened it (left or
 * right) is shifted along the screen by Radix itself as its content grows, and is left alone.
 */
import { type CSSProperties, type RefObject, useCallback, useLayoutEffect, useRef, useState } from 'react';

import { FLOATING_MIN_HEIGHT, FLOATING_MIN_ROWS, layerRoom } from '@/ui/floating';

export interface LayerPlace {
  side: 'top' | 'bottom' | 'left' | 'right';
  /** The room on that side, which the layer's own height may not pass; unset until it has been measured. */
  height: number | undefined;
}

/** What the screen shows of the page, as Floating UI measures it: the visual viewport when the browser has one. */
function viewportBox(): { top: number; bottom: number } {
  const view = window.visualViewport as VisualViewport | null | undefined;
  return view ? { top: view.offsetTop, bottom: view.offsetTop + view.height } : { top: 0, bottom: document.documentElement.clientHeight };
}

const ITEMS = '[role="option"],[role="menuitem"],[role="menuitemradio"],[role="menuitemcheckbox"]';

/** What a layer is made of, as far as its height is concerned: found when what it holds changes, not at every decision. */
interface Parts {
  /** The parts that scroll its content: the layer itself, or the outermost parts inside it that the layer's height shrinks. */
  scrollers: HTMLElement[];
  /** The rows of its list: how many, and the first and the last, which stand for the rest. */
  rows: number;
  first: HTMLElement | null;
  last: HTMLElement | null;
}

const overflows = (part: HTMLElement) => part.scrollHeight > part.clientHeight + 1;
const scrolls = (part: HTMLElement) => ['auto', 'scroll'].includes(getComputedStyle(part).overflowY);

/** Finds the parts of a layer. Only a part that holds back some of its content is asked for its style, and no row is entered: a row holds no scroller. */
function findParts(layer: HTMLElement): Parts {
  const scrollers: HTMLElement[] = [];
  const walk = (parent: HTMLElement) => {
    for (const child of Array.from(parent.children)) {
      if (!(child instanceof HTMLElement) || child.matches(ITEMS)) continue;
      // A part with a height of its own (a list that holds ten rows however much room there is) is not one the layer's height shrinks.
      if (overflows(child) && scrolls(child) && ['none', ''].includes(getComputedStyle(child).maxHeight)) scrollers.push(child); else walk(child);
    }
  };
  if (overflows(layer) && scrolls(layer)) scrollers.push(layer); else walk(layer);
  const items = layer.querySelectorAll<HTMLElement>(ITEMS);
  return { scrollers, rows: items.length, first: items[0] ?? null, last: items[items.length - 1] ?? null };
}

/** The layer's whole height if nothing held it, and the least of it worth showing: its own parts and `rows` of its list, or `minimum` of a card. */
function measureLayer(layer: HTMLElement, parts: Parts, rows: number, minimum: number): { natural: number; least: number } | null {
  const shown = layer.offsetHeight;
  // A layer with no box (not drawn, or no layout to ask) has nothing to measure.
  if (!(shown > 0)) return null;
  let hidden = 0;
  for (const part of parts.scrollers) hidden += Math.max(0, part.scrollHeight - part.clientHeight);
  const natural = Math.ceil(shown + hidden);
  // A layer with no row to show (loading, nothing found, a card) is shown whole up to `minimum`. A row is the list's mean (as the first and the
  // last stand for it): the least it asks must not change with which row the pointer or the keyboard is on, or the layer would change sides under them.
  if (parts.first === null || parts.last === null) return { natural, least: Math.min(natural, minimum) };
  const mean = (parts.first.offsetHeight + parts.last.offsetHeight) / 2;
  return { natural, least: Math.min(natural, Math.ceil(Math.max(0, natural - mean * parts.rows) + Math.min(parts.rows, rows) * mean)) };
}

/** The `style` a layer takes from its place: the height it may not pass. */
export function placeStyle(place: LayerPlace): CSSProperties | undefined {
  return place.height === undefined ? undefined : { '--floating-height': `${place.height}px` } as CSSProperties;
}

export function useLayerRoom({ shown, anchor, side = 'bottom', watch, gap, rows = FLOATING_MIN_ROWS, minimum = FLOATING_MIN_HEIGHT }: {
  /** The layer is open and not hidden behind a prompt. */
  shown: boolean;
  /** What opened it. */
  anchor: RefObject<HTMLElement | null>;
  /** The side its caller wants it on. */
  side?: LayerPlace['side'];
  /** Changes when the content is replaced (it may change on every render: only a render that changed what the layer holds asks for a decision). */
  watch: unknown;
  /** The gap the layer keeps from what opened it. */
  gap: number;
  /** How many rows of its list the layer promises before it looks for room on the other side. */
  rows?: number;
  /** The least of a layer of no rows (a card), in CSS pixels. */
  minimum?: number;
}): { place: LayerPlace; attach: (node: HTMLElement | null) => void } {
  const [place, setPlace] = useState<LayerPlace>({ side, height: undefined });
  const [layer, setLayer] = useState<HTMLElement | null>(null);
  const attach = useCallback((node: HTMLElement | null) => { setLayer((was) => was === node ? was : node); }, []);
  // The side this layer was last given, once it has been given one: the first decision is made with nothing to keep.
  // Floating UI's own `data-side` is not asked: it trails a decision by a frame and would undo it.
  const stood = useRef<'above' | 'below' | null>(null);
  // What a render that may have changed the layer's content asks of the open layer: its decision at once.
  const open = useRef<{ arrived: () => void } | null>(null);
  const above = side === 'top' || side === 'bottom';

  // A layer that is put away, or hidden, is measured afresh when it comes back.
  const [wasShown, setWasShown] = useState(shown);
  if (wasShown !== shown) { setWasShown(shown); if (!shown) setPlace({ side, height: undefined }); }
  const [wasSide, setWasSide] = useState(side);
  if (wasSide !== side) { setWasSide(side); setPlace({ side, height: undefined }); }

  // Set up once for each open layer, and not at every render of it.
  useLayoutEffect(() => {
    if (!shown || !above) { stood.current = null; open.current = null; return; }
    const field = anchor.current;
    if (layer === null || field === null) return;
    let live = true, pending = 0, parts: Parts | null = null, seen = '', width = -1;
    const decide = () => {
      pending = 0;
      if (!live) return;
      parts ??= findParts(layer);
      const size = measureLayer(layer, parts, rows, minimum);
      const viewport = viewportBox();
      if (size === null || !(viewport.bottom > viewport.top)) return;
      const box = field.getBoundingClientRect();
      // Nothing it reads has changed since the last decision: the same answer, which is not asked for again.
      const reading = `${size.natural} ${size.least} ${box.top} ${box.bottom} ${viewport.top} ${viewport.bottom}`;
      if (reading === seen) return;
      seen = reading;
      const room = layerRoom({ anchor: box, viewport, ...size, ...(stood.current === null ? {} : { now: stood.current }), prefer: side === 'top' ? 'above' : 'below', gap });
      stood.current = room.above ? 'above' : 'below';
      const height = Math.floor(room.height);
      const next: LayerPlace = { side: room.above ? 'top' : 'bottom', height: height > 0 ? height : undefined };
      setPlace((was) => was.side === next.side && was.height === next.height ? was : next);
    };
    const later = () => { if (live && pending === 0) pending = requestAnimationFrame(decide); };
    // What it holds changes (rows arrive, a card's body is replaced): its parts are found again, at the next decision.
    const reparts = () => { parts = null; later(); };
    // A layer held to its height does not change size when its content grows: what arrives in it is seen as it is added.
    const arrivals = typeof MutationObserver === 'undefined' ? null : new MutationObserver(reparts);
    arrivals?.observe(layer, { childList: true, subtree: true, characterData: true });
    // What opened it, and the layer itself: its width changes how its rows wrap.
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver((entries) => {
      const now = entries.find((entry) => entry.target === layer)?.contentRect.width;
      if (now !== undefined && now !== width) { width = now; parts = null; }
      later();
    });
    observer?.observe(field);
    observer?.observe(layer);
    // The page moving under the layer (a dialog scrolled, a window resized) changes the room on each side of what opened it.
    const moved = (event: Event) => { if (!(event.target instanceof Node) || !layer.contains(event.target)) later(); };
    window.addEventListener('resize', later);
    window.visualViewport?.addEventListener('resize', later);
    window.visualViewport?.addEventListener('scroll', later);
    document.addEventListener('scroll', moved, { capture: true, passive: true });
    // An image or a frame that arrives changes the size of what is held without changing what it is made of.
    layer.addEventListener('load', reparts, true);
    // The decision that precedes the first frame of the layer: a layer drawn once as it was first made would be seen cut.
    decide();
    open.current = {
      // After a render that changed what the layer holds, again before the frame is drawn; a render that did not asks for nothing.
      arrived: () => { if (arrivals !== null && arrivals.takeRecords().length === 0) return; parts = null; decide(); },
    };
    return () => {
      live = false;
      open.current = null;
      cancelAnimationFrame(pending);
      observer?.disconnect();
      arrivals?.disconnect();
      layer.removeEventListener('load', reparts, true);
      window.removeEventListener('resize', later);
      window.visualViewport?.removeEventListener('resize', later);
      window.visualViewport?.removeEventListener('scroll', later);
      document.removeEventListener('scroll', moved, { capture: true });
    };
  }, [shown, above, layer, side, rows, minimum, gap, anchor]);

  // A render of the caller may have replaced what the layer holds: this is where it is asked whether it did.
  useLayoutEffect(() => { open.current?.arrived(); }, [watch]);

  return { place, attach };
}
