/** Shared VirtualList component (collections); primitives are wrapped here.
 *
 * A long list drawn by the rows that are in view, whose rows differ in height and change it: a
 * conversation of thousands of items scrolls as smoothly as one of ten. It begins at the end, can
 * keep following the end while its reader is at the end, leaves the reader where they are when they
 * are not, keeps the row they were looking at in place when older rows are added above it, and tells
 * its owner when the reader comes near the start or leaves the end. The following and the keeping in
 * place are the virtualizer's own (it measures the rows and corrects the scroll position in the
 * same frame, which a position kept here from the render before cannot). What it cannot do is reach
 * the end of rows it has not measured: it goes by what it estimates, and rows that are taller than
 * that grow the list under the reader after they have arrived. So a jump to the end holds on to the end
 * for as long as the rows keep being measured (the list says it is busy while it does), and lets go at
 * the first thing the reader does. And it is not led astray by a window that is taller than all the
 * rows: there is nothing to scroll then, and every row is drawn.
 */
import { useVirtualizer } from '@tanstack/react-virtual';
import { type ReactNode, type Ref, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';

export interface VirtualListHandle {
  /** Brings the end into view. */
  scrollToEnd: () => void;
  /** Brings the row with this key into view; whether there is one. */
  scrollToKey: (key: string, align?: 'start' | 'center' | 'end') => boolean;
  /** Whether the reader is at the end, within the slack. */
  isAtEnd: () => boolean;
  /** The element that scrolls. */
  element: () => HTMLDivElement | null;
}

/** A row identity and its offset survive inserting older rows or reopening the view. */
export interface VirtualListPosition { key: string | null; offset: number; atEnd: boolean }

export interface VirtualListProps {
  /** A view can own the viewport shared by its controls and this virtual content. */
  viewport?: HTMLDivElement | null;
  count: number;
  /** A key for the row, which is the same for the same row whatever its place. */
  keyOf: (index: number) => string;
  /** What a row is taken to measure until it has been drawn. */
  estimate: (index: number) => number;
  render: (index: number) => ReactNode;
  /** What the list is, for a screen reader. */
  label: string;
  /** Keep the end in view while the reader is at it. */
  follow?: boolean;
  /** Begin at the end (the default); false where something else will bring a row into view at once. */
  startAtEnd?: boolean;
  /** How far from the end still counts as at the end, in pixels. */
  endSlackPx?: number;
  /** How near the start the reader has to come to be handed older rows, in pixels. */
  nearStartPx?: number;
  onNearStart?: () => void;
  onAtEnd?: (atEnd: boolean) => void;
  /** The rows that are in view now: the first and the last. */
  onRange?: (first: number, last: number) => void;
  initialPosition?: VirtualListPosition | null;
  onPosition?: (position: VirtualListPosition) => void;
  /** How many rows beyond the view are drawn on each side. */
  overscan?: number;
  className?: string;
  ref?: Ref<VirtualListHandle>;
}

/** A reader this close to the end is at the end: a few lines of leeway, so that a trackpad's drift does not let go. */
export const END_SLACK_PX = 80;
/** Handed older rows from this near the start: about a screen. */
const NEAR_START_PX = 600;
const OVERSCAN_ROWS = 8;
/**
 * A jump to the end is held until the list has not moved for this many frames, and for at most this many frames: frames and not
 * seconds, for a machine that is slower measures the same rows in the same number of frames and more seconds (a hold of four
 * seconds let go of a list that a machine three times as slow was still measuring). The seconds are the limit of a page whose
 * frames hardly come, which would otherwise be held for ever; four seconds of the frames of a machine that is not slow is 240.
 */
const SETTLED_AFTER_FRAMES = 8;
const SETTLE_AT_MOST_FRAMES = 240;
const SETTLE_AT_MOST_MS = 30_000;
/** What the reader does, with their own hand or by moving the focus, and so what ends the hold on the end. */
const LET_GO_BY = ['wheel', 'touchstart', 'keydown', 'pointerdown', 'focusin'] as const;

export function VirtualList({
  viewport,
  count,
  keyOf,
  estimate,
  render,
  label,
  follow = true,
  startAtEnd = true,
  endSlackPx = END_SLACK_PX,
  nearStartPx = NEAR_START_PX,
  onNearStart,
  onAtEnd,
  onRange,
  initialPosition,
  onPosition,
  overscan = OVERSCAN_ROWS,
  className,
  ref,
}: VirtualListProps) {
  const content = useRef<HTMLDivElement>(null);
  const scroller = useMemo(() => viewport === undefined ? content : { current: viewport }, [viewport]);
  const [margin, setMargin] = useState(0);
  const scrollMargin = viewport === undefined ? 0 : margin;
  // A shared viewport includes the controls before this list. Their measured offset belongs
  // to the virtualizer, while the row transforms remain relative to the list's own content.
  useLayoutEffect(() => {
    if (viewport === undefined || viewport === null || content.current === null) return;
    const measure = () => {
      const element = content.current;
      if (element !== null) setMargin(element.getBoundingClientRect().top - viewport.getBoundingClientRect().top
        - viewport.clientTop + viewport.scrollTop);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    for (let at: HTMLElement | null = content.current; at !== null && at !== viewport; at = at.parentElement) {
      observer.observe(at);
      for (let before = at.previousElementSibling; before !== null; before = before.previousElementSibling) observer.observe(before);
    }
    return () => observer.disconnect();
  }, [viewport]);
  const atEnd = useRef(true);
  const positioned = useRef(false);
  const lastRange = useRef('');
  /** What holds the end in view while the rows are measured, and how to let go of it. */
  const holding = useRef<(() => void) | null>(null);

  // The virtualizer hands out functions that change with the list; this component uses them only here.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count,
    getScrollElement: () => scroller.current,
    scrollMargin,
    estimateSize: estimate,
    getItemKey: keyOf,
    overscan,
    // Rows come in above (older ones) and below (newer ones) and change height while they are drawn:
    // the reader keeps what they look at, and a reader who is at the end stays there.
    anchorTo: 'end',
    followOnAppend: follow,
    scrollEndThreshold: endSlackPx,
  });
  const total = virtualizer.getTotalSize();

  const endOffset = useCallback((element: HTMLDivElement) => {
    const maximum = Math.max(0, element.scrollHeight - element.clientHeight);
    if (viewport === undefined) return maximum;
    const tail = Math.min(element.scrollHeight, scrollMargin + virtualizer.getTotalSize());
    // Keep the conversation's existing CSS minimum visible when controls exceed the view.
    // Remaining controls stay reachable by scrolling the same owner.
    const minimum = content.current === null ? 0 : Number.parseFloat(getComputedStyle(content.current).minHeight) || 0;
    const footer = Math.max(0, element.scrollHeight - tail);
    return Math.max(0, Math.min(maximum, tail + Math.min(footer, Math.max(0, element.clientHeight - minimum)) - element.clientHeight));
  }, [viewport, scrollMargin, virtualizer]);

  /**
   * Goes to the end, and stays there while the rows that come into view are measured and change how far the end is.
   * Done here and not by the virtualizer's own scroll to an index, which keeps its target for seconds and takes the
   * list back to it whenever a row it measures moves it, against whatever else has scrolled the list in the meantime.
   */
  const toEnd = useCallback(() => {
    const element = scroller.current;
    holding.current?.();
    if (element === null) return;
    let frame = 0;
    let quiet = 0;
    let frames = 0;
    const began = performance.now();
    const release = () => {
      cancelAnimationFrame(frame);
      for (const kind of LET_GO_BY) element.removeEventListener(kind, release);
      element.removeAttribute('aria-busy');
      if (holding.current === release) holding.current = null;
    };
    holding.current = release;
    // The reader's own hand ends it: nothing here fights a person who scrolls, or a focus that is moved into the list.
    for (const kind of LET_GO_BY) element.addEventListener(kind, release, { once: true, passive: true });
    element.setAttribute('aria-busy', 'true');
    const step = () => {
      const target = endOffset(element);
      if (Math.abs(target - element.scrollTop) > 1) {
        element.scrollTop = target;
        quiet = 0;
      } else quiet += 1;
      frames += 1;
      if (quiet < SETTLED_AFTER_FRAMES && frames < SETTLE_AT_MOST_FRAMES && performance.now() - began < SETTLE_AT_MOST_MS) frame = requestAnimationFrame(step);
      else release();
    };
    // The first step is now: the rows that stand at the end are drawn from this position.
    element.scrollTop = endOffset(element);
    frame = requestAnimationFrame(step);
  }, [scroller, endOffset]);

  useImperativeHandle(
    ref,
    () => ({
      scrollToEnd: toEnd,
      scrollToKey: (key, align = 'center') => {
        if (scroller.current === null) return false;
        for (let index = 0; index < count; index += 1) {
          if (keyOf(index) === key) {
            // Somebody asked for this row: what holds the end gives way.
            holding.current?.();
            positioned.current = true;
            if (viewport !== undefined && align === 'end' && index === count - 1) {
              virtualizer.scrollToOffset(Math.max(0, scrollMargin + virtualizer.getTotalSize() - scroller.current.clientHeight));
            } else virtualizer.scrollToIndex(index, { align });
            return true;
          }
        }
        return false;
      },
      isAtEnd: () => atEnd.current,
      element: () => scroller.current,
    }),
    // The handle reads the latest rows through the closures below.
    [count, keyOf, virtualizer, toEnd, scroller, viewport, scrollMargin],
  );

  const restorePosition = useCallback((index: number, offset: number) => {
    const element = scroller.current;
    if (element === null) return;
    holding.current?.();
    const initial = virtualizer.getOffsetForIndex(index, 'start');
    if (initial === undefined) return;
    positioned.current = true;
    virtualizer.scrollToOffset(initial[0] + offset);
    let frame = 0, frames = 0, quiet = 0;
    const began = performance.now();
    const release = () => {
      cancelAnimationFrame(frame);
      for (const kind of LET_GO_BY) element.removeEventListener(kind, release);
      element.removeAttribute('aria-busy');
      if (holding.current === release) holding.current = null;
    };
    holding.current = release;
    for (const kind of LET_GO_BY) element.addEventListener(kind, release, { once: true, passive: true });
    element.setAttribute('aria-busy', 'true');
    const step = () => {
      const row = element.querySelector<HTMLElement>(`[data-index="${index}"]`);
      if (row !== null) {
        const delta = row.getBoundingClientRect().top - element.getBoundingClientRect().top - element.clientTop + offset;
        const previous = element.scrollTop;
        element.scrollTop += delta;
        quiet = Math.abs(element.scrollTop - previous) < 1 ? quiet + 1 : 0;
      }
      frames += 1;
      if (quiet < SETTLED_AFTER_FRAMES && frames < SETTLE_AT_MOST_FRAMES && performance.now() - began < SETTLE_AT_MOST_MS) frame = requestAnimationFrame(step);
      else release();
    };
    frame = requestAnimationFrame(step);
  }, [virtualizer, scroller]);

  // The first rows: begin at the end, which is where a conversation is read from. The virtualizer puts
  // its own idea of the offset back when its effects run again (a development build runs them twice),
  // so the beginning is done again whenever the effects start anew.
  useLayoutEffect(() => {
    if (count === 0) {
      positioned.current = false;
      return;
    }
    if (scroller.current === null) return;
    if (!positioned.current && initialPosition !== undefined && initialPosition !== null && !initialPosition.atEnd && initialPosition.key !== null) {
      for (let index = 0; index < count; index += 1) if (keyOf(index) === initialPosition.key) {
        restorePosition(index, initialPosition.offset);
        break;
      }
    }
    if (follow && startAtEnd && !positioned.current) {
      positioned.current = true;
      toEnd();
    }
  });
  useLayoutEffect(
    () => () => {
      positioned.current = false;
      holding.current?.();
    },
    [],
  );

  // The virtualizer carries the position it believes the list to be at forward by the rows it measures, on the belief that the
  // element follows. A list shorter than its window cannot follow (there is nothing to scroll), and one that is taller by a
  // little follows no further than that; no scroll comes to say so, and the belief stays beyond the end: the rows drawn are
  // those of a place the list was never at, the last ones, and the rest of the window stays blank. The element's own scroll is
  // how the virtualizer learns where the list is, so it is given one when the two disagree and the belief is further than
  // the list is long. (Not when the belief is merely ahead: a position that waits for the list to grow is the virtualizer's own.)
  // Not inside the commit: the virtualizer draws again at once on a scroll, and React does not flush from inside a commit.
  const resync = useCallback(() => {
    const element = scroller.current;
    const believed = virtualizer.scrollOffset;
    if (element === null || believed === null) return false;
    const furthest = viewport === undefined ? Math.max(0, virtualizer.getTotalSize() - (virtualizer.scrollRect?.height ?? 0))
      : Math.max(0, element.scrollHeight - element.clientHeight);
    return believed > furthest + 1 && believed > element.scrollTop + 1;
  }, [virtualizer, scroller, viewport]);
  const resyncing = useRef(false);
  useLayoutEffect(() => {
    if (resyncing.current || !resync()) return;
    resyncing.current = true;
    queueMicrotask(() => {
      resyncing.current = false;
      if (resync()) scroller.current?.dispatchEvent(new Event('scroll'));
    });
  });

  const checkEnd = useCallback((element: HTMLDivElement) => {
    const tail = viewport === undefined ? element.scrollHeight : Math.min(element.scrollHeight, scrollMargin + virtualizer.getTotalSize());
    const near = tail - element.scrollTop - element.clientHeight <= endSlackPx
      && (viewport === undefined || tail > element.scrollTop);
    if (near !== atEnd.current) {
      atEnd.current = near;
      onAtEnd?.(near);
    }
    return near;
  }, [endSlackPx, onAtEnd, viewport, scrollMargin, virtualizer]);

  // The list itself gets smaller or larger (the composer under it grows, a banner comes): a reader who
  // was at the end stays at it.
  useEffect(() => {
    const element = scroller.current;
    if (element === null) return;
    // A link to the first row may already be in position and never emit a scroll event.
    checkEnd(element);
    if (!follow || typeof ResizeObserver === 'undefined') return;
    let height = element.clientHeight, extent = element.scrollHeight;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      if (element.clientHeight === height && (viewport === undefined || element.scrollHeight === extent)) return;
      height = element.clientHeight; extent = element.scrollHeight;
      // Not inside the observer's own turn: a scroll made there is reported as a loop it did not finish.
      // And asked again when the frame comes, for the reader may have gone elsewhere by then. The rows are measured
      // by now: this is one step to the end, not a hold on it.
      frame = requestAnimationFrame(() => {
        if (atEnd.current) element.scrollTop = endOffset(element);
      });
    });
    observer.observe(element);
    if (viewport !== undefined) for (const child of element.children) observer.observe(child);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [checkEnd, follow, scroller, viewport, endOffset]);

  const check = useCallback(() => {
    const element = scroller.current;
    if (element === null) return;
    const near = checkEnd(element);
    if (positioned.current && onPosition !== undefined) {
      const first = virtualizer.getVirtualItems().find((row) => row.end > element.scrollTop);
      onPosition({ key: near || first === undefined ? null : keyOf(first.index), offset: first === undefined ? 0 : Math.max(0, element.scrollTop - first.start), atEnd: near });
    }
    if (element.scrollTop < nearStartPx) onNearStart?.();
  }, [checkEnd, nearStartPx, onNearStart, onPosition, keyOf, virtualizer, scroller]);

  useEffect(() => {
    if (viewport === undefined || viewport === null) return;
    viewport.addEventListener('scroll', check, { passive: true });
    return () => viewport.removeEventListener('scroll', check);
  }, [viewport, check]);

  // Which rows are in view, for whoever asked.
  const range = virtualizer.range;
  useLayoutEffect(() => {
    if (range === null || onRange === undefined) return;
    const owner = scroller.current;
    const visible = viewport === undefined || owner === null ? null : virtualizer.getVirtualItems()
      .filter((row) => row.end > owner.scrollTop && row.start < owner.scrollTop + owner.clientHeight);
    const first = visible === null ? range.startIndex : visible[0]?.index ?? 0;
    const last = visible === null ? range.endIndex : visible.at(-1)?.index ?? -1;
    const said = `${first}.${last}`;
    if (said === lastRange.current) return;
    lastRange.current = said;
    onRange(first, last);
  });

  return (
    <div
      ref={content}
      role="log"
      aria-label={label}
      aria-relevant="additions"
      aria-atomic="false"
      tabIndex={0}
      onScroll={viewport === undefined ? check : undefined}
      className={className}
    >
      <ol role="list" className="m-0 list-none p-0" style={{ height: total, position: 'relative', width: '100%' }}>
        {virtualizer.getVirtualItems().map((row) => (
          <li
            key={row.key}
            data-index={row.index}
            aria-posinset={row.index + 1}
            aria-setsize={count}
            ref={virtualizer.measureElement}
            style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${row.start - scrollMargin}px)` }}
          >
            {render(row.index)}
          </li>
        ))}
      </ol>
    </div>
  );
}
