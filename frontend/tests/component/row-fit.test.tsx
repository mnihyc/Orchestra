/** A row of acts that never wraps: what does not fit is hidden (not moved), the last first, and comes back when there is room. */
import { act, cleanup, render, screen } from '@testing-library/react';
import { useRef } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useRowFit } from '@/features/thread/rowFit';

const observers: (() => void)[] = [];
let width: PropertyDescriptor | undefined;
let rect: PropertyDescriptor | undefined;

beforeEach(() => {
  observers.length = 0;
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { observers.push(callback); }
    observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn();
  });
  // The page has no layout here: a row is as wide as it says, and every item is as wide as it says.
  width = Object.getOwnPropertyDescriptor(Element.prototype, 'clientWidth');
  rect = Object.getOwnPropertyDescriptor(Element.prototype, 'getBoundingClientRect');
  Object.defineProperty(Element.prototype, 'clientWidth', { configurable: true, get(this: Element) { return Number(this.getAttribute('data-room') ?? 0); } });
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', { configurable: true, writable: true, value(this: Element) {
    const wide = Number(this.getAttribute('data-width') ?? 0);
    return new DOMRect(0, 0, wide, 28);
  } });
});
afterEach(() => {
  cleanup();
  if (width !== undefined) Object.defineProperty(Element.prototype, 'clientWidth', width);
  if (rect !== undefined) Object.defineProperty(Element.prototype, 'getBoundingClientRect', rect);
  vi.unstubAllGlobals();
});

function Row({ room, items }: { room: number; items: Record<string, number> }) {
  const row = useRef<HTMLDivElement>(null);
  const hidden = useRowFit(row, Object.keys(items));
  return <div ref={row} data-room={room} data-testid="row">
    <div data-fold-title data-width="0" />
    <span data-fold-fixed data-width="30" />
    {Object.entries(items).map(([id, size]) => <span key={id} data-fold={id} data-width={size} hidden={hidden.has(id)}>{id}</span>)}
  </div>;
}

const ITEMS = { changes: 20, find: 30, view: 40 };
const shown = () => ['changes', 'find', 'view'].filter((id) => screen.getByTestId('row').querySelector(`[data-fold="${id}"]`)?.hasAttribute('hidden') === false);

it('hides the last act first when the row is too narrow for all of them', () => {
  render(<Row room={100} items={ITEMS} />);
  // 100 less the 30 that always stays leaves 70 for 20 + 30 + 40.
  expect(shown()).toEqual(['changes', 'find']);
});

it('keeps every act while they fit, and none of them is hidden before the row has a width', () => {
  render(<Row room={160} items={ITEMS} />);
  expect(shown()).toEqual(['changes', 'find', 'view']);
  cleanup();
  render(<Row room={0} items={ITEMS} />);
  expect(shown()).toEqual(['changes', 'find', 'view']);
});

it('brings a hidden act back when the row grows, with the width it had when it was last drawn', () => {
  render(<Row room={160} items={ITEMS} />);
  const row = screen.getByTestId('row');
  row.setAttribute('data-room', '100');
  act(() => { for (const observe of observers) observe(); });
  expect(shown()).toEqual(['changes', 'find']);
  row.setAttribute('data-room', '45');
  act(() => { for (const observe of observers) observe(); });
  expect(shown()).toEqual([]);
  row.setAttribute('data-room', '160');
  act(() => { for (const observe of observers) observe(); });
  expect(shown()).toEqual(['changes', 'find', 'view']);
});
