/** The real popup can mount and acquire its available height after the input's open commit. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ComponentProps, useState } from 'react';
import type * as Radix from 'radix-ui';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Combobox } from '@/ui/Combobox';
import { show } from './support';

const portal = vi.hoisted(() => ({ mounted: false, listeners: new Set<() => void>() }));
vi.mock('radix-ui', async (load) => {
  const actual = await load<typeof Radix>();
  const { useSyncExternalStore } = await import('react');
  const NativePortal = actual.Popover.Portal;
  // Defer only portal mounting. Actual Content, focus/ARIA and positioning primitives remain in use.
  function DelayedPortal(props: ComponentProps<typeof NativePortal>) {
    const mounted = useSyncExternalStore((notify) => {
      portal.listeners.add(notify);
      return () => { portal.listeners.delete(notify); };
    }, () => portal.mounted);
    return mounted ? <NativePortal {...props} /> : null;
  }
  return { ...actual, Popover: { ...actual.Popover, Portal: DelayedPortal } };
});

const observers = new Set<Sizes>();
let pageTop = 0;
class Sizes implements ResizeObserver {
  readonly targets = new Set<Element>();
  private readonly callback: ResizeObserverCallback;
  constructor(callback: ResizeObserverCallback) { this.callback = callback; observers.add(this); }
  observe(target: Element) { this.targets.add(target); }
  unobserve(target: Element) { this.targets.delete(target); }
  disconnect() { this.targets.clear(); }
  deliver() { this.callback([], this); }
}

const options = Array.from({ length: 13 }, (_, index) => ({ value: `remote-${index + 1}`, label: `Remote ${index + 1}`, text: `Remote ${index + 1}` }));
function Example({ footer = false, changed = () => undefined }: { footer?: boolean; changed?: (value: string) => void }) {
  const [query, setQuery] = useState('');
  const [value, setValue] = useState('remote-13');
  return <div data-testid="outer-scroller">
    <Combobox label="Remote" query={query} onQueryChange={setQuery} initialActiveValue={value}
      displayValue={options.find((option) => option.value === value)?.text}
      groups={[{ id: 'remotes', label: 'Remotes', options: options.filter((option) => option.value.includes(query)) }]}
      onSelect={(option) => { changed(option.value); setValue(option.value); }}
      footer={footer ? <button type="button">Next page</button> : undefined} />
  </div>;
}

beforeEach(() => {
  pageTop = document.documentElement.scrollTop;
  portal.mounted = false;
  observers.clear();
  vi.stubGlobal('ResizeObserver', Sizes);
});
afterEach(() => {
  cleanup();
  document.documentElement.scrollTop = pageTop;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function mountPortal() {
  act(() => { portal.mounted = true; for (const notify of portal.listeners) notify(); });
}

/** jsdom has no layout: deliver the measured mount, placement and wrapping sizes explicitly. */
function geometry(list: HTMLElement, footer: boolean, scale = 1) {
  const frame = list.closest('[data-radix-popper-content-wrapper]')?.firstElementChild;
  if (!(frame instanceof HTMLElement)) throw new Error('Actual Radix Content is missing');
  const viewport = footer ? list : frame;
  const size = { height: 520, row: 40 };
  const rect = (top: number, height: number) => ({ top, bottom: top + height, left: 0, right: 300, width: 300, height, x: 0, y: top, toJSON: () => ({}) });
  Object.defineProperties(viewport, {
    clientHeight: { configurable: true, get: () => size.height },
    offsetHeight: { configurable: true, get: () => size.height + 2 },
    clientTop: { configurable: true, value: 1 },
    scrollHeight: { configurable: true, get: () => within(list).queryAllByRole('option').length * size.row },
  });
  vi.spyOn(viewport, 'getBoundingClientRect').mockImplementation(() => rect(200, (size.height + 2) * scale));
  for (const option of within(list).getAllByRole('option')) {
    vi.spyOn(option, 'getBoundingClientRect').mockImplementation(() => {
      const index = within(list).getAllByRole('option').indexOf(option);
      return rect(200 + scale + (index * size.row - viewport.scrollTop) * scale, size.row * scale);
    });
  }
  const deliver = (target: Element = viewport) => act(() => {
    // Native Popper observes its wrapper/anchor; deliver only the viewport/group whose size changed.
    for (const observer of observers) if (observer.targets.has(target)) observer.deliver();
  });
  return { frame, viewport, size, deliver };
}

it.each([
  { footer: false, scale: 1 },
  { footer: false, scale: 0.98 },
  { footer: true, scale: 0.98 },
])('reveals the unchanged late current item after delayed portal mount and placement ($footer, $scale)', async ({ footer, scale }) => {
  const user = userEvent.setup(), changed = vi.fn(), globalScroll = vi.spyOn(Element.prototype, 'scrollIntoView');
  show(<Example footer={footer} changed={changed} />);
  const outer = screen.getByTestId('outer-scroller'); outer.scrollTop = 71;
  document.documentElement.scrollTop = 43;
  const input = screen.getByRole('combobox', { name: 'Remote' });
  await user.click(input);
  const active = input.getAttribute('aria-activedescendant');
  expect(active).toBeTruthy(); expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  mountPortal();
  const list = await screen.findByRole('listbox', { name: 'Remote' });
  const measured = geometry(list, footer, scale);
  measured.deliver(); expect(measured.viewport.scrollTop).toBeCloseTo(0);
  // Positioning constrains an already-mounted popup without changing open/current/query.
  measured.size.height = 160; measured.deliver();
  await waitFor(() => expect(measured.viewport.scrollTop).toBeCloseTo(360));
  expect(input).toHaveAttribute('aria-activedescendant', active);
  expect(within(list).getByRole('option', { name: 'Remote 13' })).toHaveAttribute('aria-selected', 'true');
  expect(input).toHaveFocus(); expect(changed).not.toHaveBeenCalled();
  expect(outer.scrollTop).toBe(71); expect(document.documentElement.scrollTop).toBe(43);
  expect(globalScroll).not.toHaveBeenCalled();
  if (footer) expect(measured.frame.scrollTop).toBe(0);
  // Later wrapped descriptions change row positions while the selected identity stays the same.
  measured.size.row = 60; measured.deliver(list.firstElementChild ?? list);
  expect(measured.viewport.scrollTop).toBeCloseTo(620);
});

it('keeps keyboard identity, query changes and disposal scoped to the open popup', async () => {
  const user = userEvent.setup(), changed = vi.fn(), globalScroll = vi.spyOn(Element.prototype, 'scrollIntoView');
  show(<Example changed={changed} />);
  const input = screen.getByRole('combobox', { name: 'Remote' });
  await user.click(input); mountPortal();
  const list = await screen.findByRole('listbox', { name: 'Remote' });
  const measured = geometry(list, false);
  measured.size.height = 160; measured.deliver();
  await waitFor(() => expect(measured.viewport.scrollTop).toBeCloseTo(360));
  await user.keyboard('{Home}'); expect(measured.viewport.scrollTop).toBeCloseTo(0);
  expect(input).toHaveAttribute('aria-activedescendant', within(list).getByRole('option', { name: 'Remote 1' }).id);
  await user.keyboard('{End}'); expect(measured.viewport.scrollTop).toBeCloseTo(360);
  expect(input).toHaveAttribute('aria-activedescendant', within(list).getByRole('option', { name: 'Remote 13' }).id);
  await user.type(input, 'remote-12');
  expect(input).toHaveAttribute('aria-activedescendant', within(list).getByRole('option', { name: 'Remote 12' }).id);
  expect(measured.viewport.scrollTop).toBeCloseTo(0);
  const pending = [...observers].filter((observer) => observer.targets.has(list));
  expect(pending.length).toBeGreaterThan(0);
  await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith('remote-12');
  await waitFor(() => expect(screen.queryByRole('listbox')).not.toBeInTheDocument());
  expect(input).toHaveValue('Remote 12'); expect(input).not.toHaveAttribute('aria-activedescendant');
  const closedScroll = measured.viewport.scrollTop;
  measured.size.height = 40;
  act(() => { for (const observer of pending) observer.deliver(); });
  expect(measured.viewport.scrollTop).toBe(closedScroll);
  expect(globalScroll).not.toHaveBeenCalled();
});

it('leaves focused footer controls in place and reveals the current row when focus returns to the input', async () => {
  const user = userEvent.setup();
  show(<Example footer />);
  const input = screen.getByRole('combobox', { name: 'Remote' });
  await user.click(input); mountPortal();
  const list = await screen.findByRole('listbox', { name: 'Remote' });
  const measured = geometry(list, true);
  measured.size.height = 160; measured.deliver();
  await waitFor(() => expect(measured.viewport.scrollTop).toBeCloseTo(360));
  await user.tab(); expect(screen.getByRole('button', { name: 'Next page' })).toHaveFocus();
  measured.viewport.scrollTop = 0;
  measured.size.height = 80; measured.deliver();
  expect(measured.viewport.scrollTop).toBe(0);
  await user.tab({ shift: true }); expect(input).toHaveFocus();
  expect(measured.viewport.scrollTop).toBeCloseTo(440);
  expect(measured.frame.scrollTop).toBe(0);
});
