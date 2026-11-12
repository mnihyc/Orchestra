/** A popup's side and height follow the room for the content it has now, not the content it opened with. */
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Combobox } from '@/ui/Combobox';
import { show } from './support';

// jsdom has no layout: a row is 40 px; the popup has 2 px of border and 8 of padding, and a footer is 40 more.
const ROW = 40, BORDER = 2, PADDING = 8, FOOTER = 40;
let screenHeight = 900, fieldBottom = 806;

const frameOf = (node: HTMLElement) => node.parentElement?.hasAttribute('data-radix-popper-content-wrapper') === true;
const rowsIn = (frame: HTMLElement) => frame.querySelectorAll('[role="option"]').length;
const hasFooter = (frame: HTMLElement) => frame.querySelector('[data-combobox-footer]') !== null;
const wholeOf = (frame: HTMLElement) => rowsIn(frame) * ROW + BORDER + PADDING + (hasFooter(frame) ? FOOTER : 0);
/** What a popup held to its `--floating-height` shows of itself. */
const shownOf = (frame: HTMLElement) => Math.min(wholeOf(frame), Number.parseFloat(frame.style.getPropertyValue('--floating-height')) || Infinity);

beforeEach(() => {
  screenHeight = 900; fieldBottom = 806;
  vi.stubGlobal('innerHeight', screenHeight);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    if (this.getAttribute('role') === 'option') return ROW;
    if (this.hasAttribute('data-combobox-options')) return rowsIn(this) * ROW;
    return frameOf(this) ? shownOf(this) : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
    if (frameOf(this)) return shownOf(this) - BORDER;
    // The list of a popup with a footer is what is left of the popup after its other parts.
    const frame = this.closest('[data-radix-popper-content-wrapper]')?.firstElementChild;
    return this.hasAttribute('data-combobox-options') && frame instanceof HTMLElement ? shownOf(frame) - BORDER - PADDING - FOOTER : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(function (this: HTMLElement) {
    if (frameOf(this)) return wholeOf(this) - BORDER;
    return this.hasAttribute('data-combobox-options') ? rowsIn(this) * ROW : 0;
  });
});
afterEach(() => { vi.unstubAllGlobals(); });

function Example({ count, footer = false }: { count: number; footer?: boolean }) {
  const [query, setQuery] = useState('');
  const options = Array.from({ length: count }, (_, index) => ({ value: `org-${index + 1}`, label: `Org ${index + 1}`, text: `Org ${index + 1}` }));
  return <Combobox label="Org" query={query} onQueryChange={setQuery} groups={[{ id: 'orgs', label: '', options }]} onSelect={() => undefined}
    footer={footer ? <button type="button">Show more</button> : undefined} />;
}

async function open(count: number, footer: boolean, bottom: number) {
  fieldBottom = bottom;
  const view = show(<Example count={count} footer={footer} />);
  const input = screen.getByRole('combobox', { name: 'Org' });
  // The field's own box: what the popup is measured against.
  vi.spyOn(input.parentElement!, 'getBoundingClientRect').mockImplementation(() => new DOMRect(16, fieldBottom - 36, 300, 36));
  await userEvent.setup().click(input);
  const frame = await waitFor(() => {
    const popup = document.querySelector('[data-radix-popper-content-wrapper]')?.firstElementChild;
    if (!(popup instanceof HTMLElement)) throw new Error('The popup has not opened');
    return popup;
  });
  // jsdom has no stylesheet: say which part scrolls, as the classes of the real popup do (the popup, or with a footer the list in it).
  frame.style.overflowY = footer ? 'hidden' : 'auto';
  const list = frame.querySelector<HTMLElement>('[data-combobox-options]');
  if (footer && list !== null) list.style.overflowY = 'auto';
  return { view, input, frame, grow: (next: number) => view.rerender(<Example count={next} footer={footer} />) };
}
const side = (frame: HTMLElement) => frame.getAttribute('data-side');
const room = (frame: HTMLElement) => frame.style.getPropertyValue('--floating-height');

it.each([{ footer: false }, { footer: true }])('moves above a field low on the screen when its list arrives after it has opened ($footer)', async ({ footer }) => {
  // Little room below the field (a row and a footer still fit), a screen's worth above. One row is shown at first.
  const bottom = footer ? 796 : 806, below = `${900 - bottom - 12}px`, above = `${bottom - 36 - 12}px`;
  const { frame, grow } = await open(1, footer, bottom);
  await waitFor(() => expect(side(frame)).toBe('bottom'));
  expect(room(frame)).toBe(below);
  // Thirteen rows arrive: more than the room below holds, and more than the four rows it promises.
  act(() => grow(13));
  await waitFor(() => expect(side(frame)).toBe('top'));
  expect(room(frame)).toBe(above);
  // The popup ends where the room is, with all of the list in it: the active row is whole.
  expect(shownOf(frame)).toBe(wholeOf(frame));
  // A shorter list does not bring it back below the field while a person is choosing.
  act(() => grow(2));
  await waitFor(() => expect(room(frame)).toBe(above));
  expect(side(frame)).toBe('top');
});

it('ends where the room is when its list is longer than either side', async () => {
  const { frame, grow } = await open(1, false, 806);
  act(() => grow(80));
  await waitFor(() => expect(side(frame)).toBe('top'));
  expect(room(frame)).toBe('758px');
  expect(shownOf(frame)).toBe(758);
});

it('stays below a field that has room for the rows it promises, however long its list is', async () => {
  // 252 px below the field: more than the four rows it promises, less than 13 of them.
  const { frame, grow } = await open(1, false, 636);
  act(() => grow(13));
  await waitFor(() => expect(room(frame)).toBe('252px'));
  expect(side(frame)).toBe('bottom');
  act(() => grow(40));
  await waitFor(() => expect(room(frame)).toBe('252px'));
  expect(side(frame)).toBe('bottom');
});

it('follows the window being resized and the page being scrolled under it', async () => {
  const { frame } = await open(3, false, 436);
  await waitFor(() => expect(side(frame)).toBe('bottom'));
  // The window loses height: 112 px of room below, less than the 130 px of a short list that is shown whole.
  screenHeight = 560; vi.stubGlobal('innerHeight', screenHeight);
  act(() => { window.dispatchEvent(new Event('resize')); });
  await waitFor(() => expect(side(frame)).toBe('top'));
  expect(room(frame)).toBe('388px');
});

it('follows a dialog scrolled under the field', async () => {
  const { frame } = await open(3, false, 436);
  await waitFor(() => expect(side(frame)).toBe('bottom'));
  fieldBottom = 816;
  act(() => { fireEvent.scroll(document.body); });
  await waitFor(() => expect(side(frame)).toBe('top'));
  expect(room(frame)).toBe('768px');
});
