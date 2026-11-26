/** The list a conversation is drawn in, when it is opened at its end: it holds on to the end while the rows that come into view are measured, and lets go when the list has stopped moving. */
import { act, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { VirtualList } from '@/ui/VirtualList';

/** How tall the window of the list is. */
const WINDOW_PX = 600;

/**
 * The list as the page sees it, without a browser: a content that grows as its rows are measured, a position that cannot be further
 * than the end of it, and frames that come when the test says, each after the time that a machine takes for one.
 */
function machine(owner?: HTMLElement, rowHeight?: number) {
  const content = { height: 2_000 };
  const observers = new Set<() => void>();
  let viewport = WINDOW_PX;
  let top = 0;
  let now = 0;
  let waiting: { id: number; callback: FrameRequestCallback }[] = [];
  let ids = 0;
  const furthest = () => Math.max(0, content.height - viewport);
  vi.spyOn(Element.prototype, 'scrollHeight', 'get').mockImplementation(() => content.height);
  vi.spyOn(Element.prototype, 'clientHeight', 'get').mockImplementation(() => viewport);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) { return this.hasAttribute('data-index') && rowHeight !== undefined ? rowHeight : viewport; });
  vi.spyOn(Element.prototype, 'scrollTop', 'get').mockImplementation(function (this: Element) { return owner === undefined || this === owner ? top : 0; });
  vi.spyOn(Element.prototype, 'scrollTop', 'set').mockImplementation(function (this: Element, value: number) {
    if (owner !== undefined && this !== owner) return;
    top = Math.max(0, Math.min(value, furthest()));
  });
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    ids += 1;
    waiting.push({ id: ids, callback });
    return ids;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => {
    waiting = waiting.filter((entry) => entry.id !== id);
  });
  vi.stubGlobal('ResizeObserver', class implements ResizeObserver {
    private readonly report: () => void;
    constructor(callback: ResizeObserverCallback) { this.report = () => callback([], this); }
    observe() { observers.add(this.report); }
    unobserve() { observers.delete(this.report); }
    disconnect() { observers.delete(this.report); }
  });
  return {
    resize: (height: number) => {
      viewport = height;
      for (const report of observers) report();
    },
    /** The rows measured so far make the content this much taller. */
    grow: (pixels: number) => {
      content.height += pixels;
    },
    /** How far the end is from where the list stands. */
    distanceFromEnd: () => furthest() - top,
    /** A frame, after the time it took to come. */
    frame: (milliseconds: number) => {
      now += milliseconds;
      const due = waiting;
      waiting = [];
      for (const entry of due) entry.callback(now);
    },
  };
}

function drawn(startAtEnd = true) {
  render(<VirtualList count={40} keyOf={(index) => `row-${index}`} estimate={() => 50} render={(index) => <p>Row {index}</p>} label="Conversation" startAtEnd={startAtEnd} />);
  return screen.getByRole('log', { name: 'Conversation' });
}

it('keeps the body of a measured row as the list redraws and replaces it when the row renderer changes', () => {
  machine();
  const row = vi.fn((index: number) => <p>Row {index}</p>);
  const props = { count: 20, keyOf: (index: number) => String(index), estimate: () => 50, label: 'Conversation', startAtEnd: false };
  const list = render(<VirtualList {...props} render={row} />);
  expect(screen.getByText('Row 0')).toBeVisible();
  row.mockClear();
  list.rerender(<VirtualList {...props} render={row} className="min-h-0" />);
  expect(row).not.toHaveBeenCalled();
  list.rerender(<VirtualList {...props} render={(index) => <p>Updated {index}</p>} />);
  expect(screen.getByText('Updated 0')).toBeVisible();
});

it('keeps the first row in view when the list resizes before a scroll event', () => {
  const layout = machine();
  const log = drawn(false);
  expect(log.scrollTop).toBe(0);
  act(() => {
    layout.resize(WINDOW_PX - 50);
    layout.frame(16);
  });
  expect(log.scrollTop).toBe(0);
});

describe('a list opened at its end', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it('goes on holding the end while rows are still being measured, however few frames a slow machine draws in a second', () => {
    const slow = machine();
    const log = drawn();
    expect(log).toHaveAttribute('aria-busy', 'true');
    // Two frames in a second: thirty frames of rows measured, each making the conversation taller, a quarter of a minute in all.
    for (let frames = 0; frames < 30; frames += 1) {
      slow.grow(300);
      slow.frame(500);
      expect(slow.distanceFromEnd()).toBe(0);
    }
    // The rows have all been measured: the list is at the end, and lets go of it once nothing moves any more.
    for (let frames = 0; frames < 12; frames += 1) slow.frame(500);
    expect(slow.distanceFromEnd()).toBe(0);
    expect(log).not.toHaveAttribute('aria-busy');
  });

  it.each([
    ['at the rate of a machine that is not slow', 16],
    ['on a page whose frames hardly come', 2_000],
  ])('does not hold on for ever to a list that never stops changing, %s', (_, milliseconds) => {
    const endless = machine();
    const log = drawn();
    for (let frames = 0; frames < 400; frames += 1) {
      endless.grow(10);
      endless.frame(milliseconds);
    }
    expect(log).not.toHaveAttribute('aria-busy');
  });
});


it('follows measured growth in the containing view and lets its reader stop the follow from that same viewport', () => {
  const viewport = document.createElement('div'); document.body.appendChild(viewport);
  const slow = machine(viewport);
  const view = render(<VirtualList viewport={viewport} count={40} keyOf={(index) => `row-${index}`} estimate={() => 50}
    render={(index) => <p>Row {index}</p>} label="Shared conversation" />, { container: viewport });
  try {
    const log = screen.getByRole('log', { name: 'Shared conversation' });
    expect(slow.distanceFromEnd()).toBe(0); expect(log.scrollTop).toBe(0);
    expect(viewport).toHaveAttribute('aria-busy', 'true');
    for (let frame = 0; frame < 3; frame += 1) {
      slow.grow(300); slow.frame(500); expect(slow.distanceFromEnd()).toBe(0);
    }
    viewport.dispatchEvent(new Event('wheel'));
    expect(viewport).not.toHaveAttribute('aria-busy');
    slow.grow(300); slow.frame(500); expect(slow.distanceFromEnd()).toBe(300);
    expect(log.scrollTop).toBe(0);
  } finally { view.unmount(); viewport.remove(); }
});

it('says it is at its end again when rows that come under a view held still reach it, without a scroll event', () => {
  const viewport = document.createElement('div'); document.body.appendChild(viewport);
  machine(viewport);
  // A sent message stands at the top of the view: the rows end above it, and the list does not follow.
  viewport.scrollTop = 500;
  const ends: boolean[] = [];
  // The same callbacks across renders, as the conversation gives them: nothing but the rows changes.
  const onAtEnd = (at: boolean) => { ends.push(at); };
  const keyOf = (index: number) => `row-${index}`;
  const estimate = () => 50;
  const row = (index: number) => <p>Row {index}</p>;
  const list = (count: number) => <VirtualList viewport={viewport} count={count} keyOf={keyOf} estimate={estimate}
    render={row} label="Held conversation" follow={false} startAtEnd={false} onAtEnd={onAtEnd} />;
  const view = render(list(2), { container: viewport });
  try {
    expect(ends.at(-1)).toBe(false);
    // The reply comes as rows under the held message, short enough to end in the view.
    view.rerender(list(20));
    expect(ends.at(-1)).toBe(true);
  } finally { view.unmount(); viewport.remove(); }
});

it('mounts the latest rows before the browser reports the initial scroll of a shared viewport', () => {
  const viewport = document.createElement('div');
  document.body.appendChild(viewport);
  machine(viewport, 20);
  const row = vi.fn((index: number) => <p>Row {index}</p>);
  const view = render(<VirtualList viewport={viewport} count={100} keyOf={String}
    estimate={() => 20} render={row} label="History" />, { container: viewport });
  try {
    // The viewport is 600 px tall and each row is 20 px. The oldest row is far beyond the buffer.
    expect(screen.getByText('Row 99')).toBeVisible();
    expect(row).not.toHaveBeenCalledWith(0);
  } finally { view.unmount(); viewport.remove(); }
});

it('pages from the newest of 400 items to the first turn while folded pages keep the reader at the upper edge', () => {
  const viewport = document.createElement('div');
  document.body.appendChild(viewport);
  const layout = machine(viewport, 20);
  viewport.scrollTo = (options?: ScrollToOptions | number) => { if (typeof options === 'object' && options.top !== undefined) viewport.scrollTop = options.top; };
  const items = Array.from({ length: 400 }, (_, index) => index + 1);
  let loaded = items.slice(-100);
  const before: number[] = [];
  const loadPage = () => {
    const cursor = loaded[0]!;
    if (cursor === 1 || before.at(-1) === cursor) return;
    before.push(cursor);
  };
  const list = () => {
    // The latest turn is expanded. Earlier work is folded into groups of 25 items.
    const rows = loaded.filter((id) => id >= 301 || id % 25 === 1);
    return <VirtualList viewport={viewport} count={rows.length} keyOf={(i) => String(rows[i])}
      render={(i) => <span>History item {rows[i]}</span>} estimate={() => 20} label="History"
      olderCursor={loaded[0] === 1 ? undefined : loaded[0]} onNearStart={loadPage} />;
  };
  const view = render(list(), { container: viewport });
  try {
    expect(before).toEqual([]);
    act(() => {
      viewport.dispatchEvent(new Event('wheel'));
      viewport.scrollTop = 10;
      viewport.dispatchEvent(new Event('scroll'));
    });
    for (const cursor of [301, 201, 101]) {
      expect(before.at(-1)).toBe(cursor);
      const page = items.slice(cursor - 101, cursor - 1);
      loaded = [...page, ...loaded];
      layout.grow(80);
      view.rerender(list());
      act(() => layout.frame(16));
      // Each page adds four visible rows above item 301, at the same offset.
      expect(viewport.scrollTop).toBe(10 + (301 - loaded[0]!) / 100 * 80);
    }
    expect(before).toEqual([301, 201, 101]);
    act(() => { viewport.scrollTop = 0; viewport.dispatchEvent(new Event('scroll')); });
    expect(screen.getByText('History item 1')).toBeVisible();
  } finally { view.unmount(); viewport.remove(); }
});

it('reads an earlier page on a touch scroll at the physical top even without a scroll event', () => {
  const owner = document.createElement('div');
  document.body.appendChild(owner);
  machine(owner);
  const earlier = vi.fn();
  const view = render(<VirtualList viewport={owner} count={1} keyOf={() => 'only'} render={() => 'Work'}
    estimate={() => 40} label="History" startAtEnd={false} olderCursor={301} onNearStart={earlier} />, { container: owner });
  try {
    act(() => {
      owner.dispatchEvent(Object.assign(new Event('touchstart'), { touches: [{ clientY: 20 }] }));
      owner.dispatchEvent(Object.assign(new Event('touchmove'), { touches: [{ clientY: 10 }] }));
    });
    expect(earlier).not.toHaveBeenCalled();
    act(() => { owner.dispatchEvent(Object.assign(new Event('touchmove'), { touches: [{ clientY: 30 }] })); });
    expect(earlier).toHaveBeenCalledOnce();
  } finally { view.unmount(); owner.remove(); }
});
