/** Every layer that opens above or below what opened it takes the room it has for the content it has now: a body that arrives after
 * it has opened, low on the screen, ends above what opened it with the room there is, and does not move again while it only shrinks. */
import { act, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useSyncExternalStore, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DetailCard } from '@/ui/DetailCard';
import { IntentCard } from '@/ui/IntentCard';
import { Menu } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';
import { Select } from '@/ui/Select';
import { show } from './support';

// jsdom has no layout: a row is 40 px, a layer's border and padding 10; the field's lower edge is 94 px above the screen's.
const ROW = 40, CHROME = 10, FIELD_TOP = 770, FIELD_BOTTOM = 806, SCREEN = 900;
const rows = { count: 1, listeners: new Set<() => void>() };
const grow = (count: number) => act(() => { rows.count = count; for (const notify of rows.listeners) notify(); });
const useRows = () => useSyncExternalStore((notify) => { rows.listeners.add(notify); return () => { rows.listeners.delete(notify); }; }, () => rows.count);

const layerOf = (node: HTMLElement) => node.parentElement?.hasAttribute('data-radix-popper-content-wrapper') === true;
const rowsIn = (layer: HTMLElement) => layer.querySelectorAll('[data-late-row],[role="menuitem"],[role="option"]').length;
const whole = (layer: HTMLElement) => rowsIn(layer) * ROW + CHROME;
const shown = (layer: HTMLElement) => Math.min(whole(layer), Number.parseFloat(layer.style.getPropertyValue('--floating-height')) || Infinity);

beforeEach(() => {
  rows.count = 1; rows.listeners.clear();
  vi.stubGlobal('innerHeight', SCREEN);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    if (this.hasAttribute('data-late-row') || this.getAttribute('role') === 'menuitem' || this.getAttribute('role') === 'option') return ROW;
    return layerOf(this) ? shown(this) : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) { return layerOf(this) ? shown(this) - 2 : 0; });
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(function (this: HTMLElement) { return layerOf(this) ? whole(this) - 2 : 0; });
});
afterEach(() => { vi.unstubAllGlobals(); });

function Late({ role }: { role?: 'menuitem' | 'option' }) {
  const count = useRows();
  return <>{Array.from({ length: count }, (_, index) => role === undefined
    ? <p key={index} data-late-row>Row {index + 1}</p> : <div key={index} role={role} data-late-row>Row {index + 1}</div>)}</>;
}
const opener = (node: HTMLElement) => {
  // Where what opened the layer stands: low on the screen.
  vi.spyOn(node, 'getBoundingClientRect').mockImplementation(() => new DOMRect(16, FIELD_TOP, 120, FIELD_BOTTOM - FIELD_TOP));
  return node;
};
const layer = async () => {
  const node = await waitFor(() => {
    const found = document.querySelector('[data-radix-popper-content-wrapper]')?.firstElementChild;
    if (!(found instanceof HTMLElement)) throw new Error('The layer has not opened');
    return found;
  });
  // jsdom has no stylesheet: say that the layer scrolls its content, as its classes do.
  node.style.overflowY = 'auto';
  return node;
};
const side = (node: HTMLElement) => node.getAttribute('data-side');
const room = (gap: number) => `${FIELD_TOP - gap - 8}px`;

const surfaces: { name: string; gap: number; draw: () => ReactNode; open: () => Promise<void> }[] = [
  { name: 'Popover', gap: 6, draw: () => <Popover label="Layer" open trigger={<button type="button">Open</button>}><Late /></Popover>, open: () => Promise.resolve() },
  { name: 'Menu', gap: 4, draw: () => <Menu label="Layer" open trigger={<button type="button">Open</button>} content={<Late role="menuitem" />} />, open: () => Promise.resolve() },
  { name: 'Select', gap: 4, draw: () => <Select label="Layer" hideLabel open value="a" onValueChange={() => undefined} options={[{ value: 'a', name: 'A' }]} />, open: () => Promise.resolve() },
  { name: 'DetailCard', gap: 6, draw: () => <DetailCard label="Layer" content={() => <Late />}>Open</DetailCard>, open: async () => { await userEvent.setup().click(document.querySelector<HTMLElement>('button')!); } },
  { name: 'IntentCard', gap: 6, draw: () => <IntentCard label="Layer" trigger={<button type="button">Open</button>}>{() => <Late />}</IntentCard>, open: async () => { await userEvent.setup().click(document.querySelector<HTMLElement>('button')!); } },
];

it.each(surfaces.filter((surface) => surface.name !== 'Select'))('$name ends above what opened it, low on the screen, when its body arrives after it has opened', async ({ gap, draw, open }) => {
  show(draw());
  opener(document.querySelector<HTMLElement>('button')!);
  await open();
  const node = await layer();
  // One row is shown at first, and it fits below: 94 px of room less the gap and the edge.
  await waitFor(() => expect(side(node)).toBe('bottom'));
  // Thirteen rows arrive: more than the room below holds, and more than the least it promises of itself.
  grow(13);
  await waitFor(() => expect(side(node)).toBe('top'));
  expect(node.style.getPropertyValue('--floating-height')).toBe(room(gap));
  // It ends where the room is, with all of its body in it.
  expect(shown(node)).toBe(whole(node));
  // A shorter body does not bring it back below while a person is reading or choosing.
  grow(2);
  await waitFor(() => expect(node.style.getPropertyValue('--floating-height')).toBe(room(gap)));
  expect(side(node)).toBe('top');
});

it('Select ends above its field, low on the screen, when its options arrive after it has opened', async () => {
  function Options() {
    const count = useRows();
    return <Select label="Layer" hideLabel open value="a" onValueChange={() => undefined}
      options={Array.from({ length: count }, (_, index) => ({ value: index === 0 ? 'a' : `o${index}`, name: `Row ${index + 1}` }))} />;
  }
  show(<Options />);
  opener(document.querySelector<HTMLElement>('[role="combobox"]')!);
  const node = await layer();
  await waitFor(() => expect(side(node)).toBe('bottom'));
  grow(13);
  await waitFor(() => expect(side(node)).toBe('top'));
  expect(node.style.getPropertyValue('--floating-height')).toBe(room(4));
  expect(shown(node)).toBe(whole(node));
});

it('sets what it watches up once for an open layer, and not again at a render of its caller that changed nothing in it', async () => {
  const added = vi.spyOn(document, 'addEventListener');
  // What a layer watches the page with: every scroll anywhere, seen in the capture phase without holding it up.
  const watchers = () => added.mock.calls.filter(([type, , options]) => type === 'scroll' && typeof options === 'object' && options.capture === true && options.passive === true).length;
  const draw = () => <Popover label="Layer" open trigger={<button type="button">Open</button>}><p data-late-row>Row 1</p></Popover>;
  const view = show(draw());
  opener(document.querySelector<HTMLElement>('button')!);
  const node = await layer();
  await waitFor(() => expect(side(node)).toBe('bottom'));
  const once = watchers();
  expect(once).toBe(1);
  // The same content, drawn again by its caller as it draws it at every render: nothing is set up again.
  for (let render = 0; render < 6; render += 1) view.rerender(draw());
  expect(watchers()).toBe(once);
});
