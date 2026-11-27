/** A row of acts that never wraps: what does not fit is hidden (not moved), the last first, and comes back when there is room. */
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { Activity, useRef } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useRowFit } from '@/features/thread/rowFit';
import { HEADER_FOLDS } from '@/features/thread/Header';

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

it('restores the last fit on a kept tab and measures its current width with the browser’s resize delivery', () => {
  const frame = (visible: boolean, room: number) => <Activity mode={visible ? 'visible' : 'hidden'}><Row room={room} items={ITEMS} /></Activity>;
  const view = render(frame(true, 100));
  expect(shown()).toEqual(['changes', 'find']);
  const item = screen.getByTestId('row').querySelector<HTMLElement>('[data-fold="find"]')!;
  view.rerender(frame(false, 100));
  const measure = vi.spyOn(item, 'getBoundingClientRect');
  try {
    view.rerender(frame(true, 160));
    expect(measure).not.toHaveBeenCalled();
    act(() => observers.at(-1)?.());
    expect(shown()).toEqual(['changes', 'find', 'view']);
    expect(measure).toHaveBeenCalled();
  } finally { measure.mockRestore(); }
});

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

it('keeps the fitted header while a control updates inside its existing box', async () => {
  render(<Row room={100} items={ITEMS} />);
  const item = screen.getByTestId('row').querySelector<HTMLElement>('[data-fold="find"]')!;
  const measure = vi.spyOn(item, 'getBoundingClientRect');
  try {
    await act(async () => { item.append(document.createElement('span')); await Promise.resolve(); });
    expect(measure).not.toHaveBeenCalled();
    item.setAttribute('data-width', '90');
    act(() => { for (const observe of observers) observe(); });
    expect(measure).toHaveBeenCalled();
    expect(shown()).toEqual(['changes']);
  } finally { measure.mockRestore(); }
});

const DYNAMIC = ['changes', 'agents', 'find', 'view'] as const;
function GrowingRow({ agents }: { agents: boolean }) {
  const row = useRef<HTMLDivElement>(null);
  const hidden = useRowFit(row, DYNAMIC);
  return <div ref={row} data-room="160">
    <div data-fold-title />
    <button type="button" data-fold-fixed data-thread-more data-width="30">More</button>
    {DYNAMIC.filter((id) => agents || id !== 'agents').map((id) => <button type="button" key={id} data-fold={id} hidden={hidden.has(id)} data-width={id === 'agents' ? 90 : ITEMS[id]}>{id}</button>)}
  </div>;
}

it('fits the first Agents control without a pane resize and moves focus to More before hiding its old control', async () => {
  const view = render(<GrowingRow agents={false} />);
  const find = screen.getByRole('button', { name: 'find' });
  act(() => find.focus());
  expect(find).toHaveFocus();
  view.rerender(<GrowingRow agents />);
  await waitFor(() => expect(find).toHaveAttribute('hidden'));
  expect(screen.getByRole('button', { name: 'agents' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'More' })).toHaveFocus();
});

it('keeps the agents of a thread in its header row longest, and lets the other acts leave first, as the row narrows', () => {
  const widths: Record<(typeof HEADER_FOLDS)[number], number> = { agents: 90, 'messages-nav': 60, messages: 32, changes: 80, find: 30, view: 90, help: 28 };
  render(<Row room={160} items={Object.fromEntries(HEADER_FOLDS.map((id) => [id, widths[id]]))} />);
  const standing = HEADER_FOLDS.filter((id) => screen.getByTestId('row').querySelector(`[data-fold="${id}"]`)?.hasAttribute('hidden') === false);
  expect(standing).toEqual(['agents']);
});
