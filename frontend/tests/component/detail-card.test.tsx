/** Inspection follows hover and keyboard highlight; keeping and dismissing it never chooses a value. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';

import { DetailCard } from '@/ui/DetailCard';
import { IntentCard } from '@/ui/IntentCard';
import { CARD_OPEN_DELAY_MS } from '@/ui/inspection';
import { Combobox } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { Select } from '@/ui/Select';
import { Menu } from '@/ui/Menu';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

/** The wait for a card to open: five of its delays. */
const CARD_WAIT = { timeout: CARD_OPEN_DELAY_MS * 5 };

it('reads nothing before hover and closes a passing card after leaving', async () => {
  const content = vi.fn(() => <span>Provider details</span>);
  const user = userEvent.setup();
  show(<DetailCard label="Provider" content={content}>Provider</DetailCard>);
  expect(content).not.toHaveBeenCalled();
  const trigger = screen.getByRole('button', { name: 'Provider' });
  await user.hover(trigger);
  // The card opens after its delay of the clock; the wait for it is five delays, so that a pause of the machine does not end it first.
  expect(await screen.findByText('Provider details', {}, CARD_WAIT)).toBeVisible();
  await user.unhover(trigger);
  await waitFor(() => expect(screen.queryByText('Provider details')).not.toBeInTheDocument(), CARD_WAIT);
});

it('opens on focus, keeps on Enter and Escape dismisses it while returning focus', async () => {
  const user = userEvent.setup();
  show(<DetailCard label="Environment" content={() => <button type="button">Open environment</button>}>Environment</DetailCard>);
  await user.tab();
  expect(await screen.findByRole('button', { name: 'Open environment' })).toBeVisible();
  await user.keyboard('{Enter}');
  expect(await screen.findByRole('dialog', { name: 'Environment' })).toBeVisible();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Environment' })).not.toBeInTheDocument());
  expect(screen.queryByRole('button', { name: 'Open environment' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Environment' })).toHaveFocus();
});

it('keeps an escaped inspection closed until a new pointer entry follows a leave', () => {
  vi.useFakeTimers();
  const view = show(<DetailCard label="Provider" content={() => <span>Provider details</span>}>Provider</DetailCard>);
  try {
    const trigger = screen.getByRole('button', { name: 'Provider' });
    act(() => { trigger.focus(); });
    expect(screen.getByText('Provider details')).toBeVisible();
    fireEvent.keyDown(trigger, { key: 'Escape' });
    act(() => { vi.runOnlyPendingTimers(); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
    fireEvent.pointerOver(trigger, { pointerType: 'mouse' });
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.runOnlyPendingTimers(); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
    fireEvent.pointerOut(trigger, { pointerType: 'mouse', relatedTarget: document.body });
    fireEvent.pointerOver(trigger, { pointerType: 'mouse', relatedTarget: document.body });
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.runOnlyPendingTimers(); });
    expect(screen.getByText('Provider details')).toBeVisible();
  } finally { view.unmount(); vi.useRealTimers(); }
});

it('keeps a clicked card when the pointer leaves and closes it on an outside press', async () => {
  const user = userEvent.setup();
  show(<><DetailCard label="Host" content={() => <span>Host details</span>}>Host</DetailCard><button type="button">Outside</button></>);
  const trigger = screen.getByRole('button', { name: 'Host' });
  await user.click(trigger); await user.unhover(trigger);
  expect(await screen.findByRole('dialog', { name: 'Host' })).toHaveTextContent('Host details');
  await user.click(screen.getByRole('button', { name: 'Outside' }));
  await waitFor(() => expect(screen.queryByText('Host details')).not.toBeInTheDocument());
});

it('follows a select highlight without selecting and Right Arrow enters its card', async () => {
  const changed = vi.fn();
  function Choice() {
    const [value, setValue] = useState('a');
    return <Select label="Model" value={value} onValueChange={(next) => { setValue(next); changed(next); }} options={[
      { value: 'a', name: 'Alpha', preview: { label: 'Alpha', content: () => <button type="button">Alpha details</button> } },
      { value: 'b', name: 'Bravo', preview: { label: 'Bravo', content: () => <button type="button">Bravo details</button> } },
    ]} />;
  }
  const user = userEvent.setup(); show(<Choice />);
  await user.click(screen.getByRole('combobox', { name: 'Model' }));
  expect(await screen.findByRole('button', { name: 'Alpha details' }, CARD_WAIT)).toBeVisible();
  await user.keyboard('{ArrowDown}');
  expect(await screen.findByRole('button', { name: 'Bravo details' }, CARD_WAIT)).toBeVisible();
  expect(changed).not.toHaveBeenCalled();
  await user.keyboard('{ArrowRight}');
  expect(await screen.findByRole('dialog', { name: 'Bravo' }, CARD_WAIT)).toBeVisible();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.getByRole('option', { name: 'Bravo' })).toHaveFocus());
  await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith('b');
});

it('chooses an option when its hint is clicked', async () => {
  const chosen = vi.fn(); const user = userEvent.setup();
  show(<Select label="Provider" value="b" onValueChange={chosen} options={[
    { value: 'a', name: 'Alpha', description: <span data-resource-hint="provider">62% used</span>, preview: { label: 'Alpha', content: () => <span>Provider details</span> } },
    { value: 'b', name: 'Bravo' },
  ]} />);
  await user.click(screen.getByRole('combobox', { name: 'Provider' }));
  await user.click(screen.getByText('62% used'));
  expect(chosen).toHaveBeenCalledExactlyOnceWith('a');
  expect(screen.queryByRole('dialog', { name: 'Alpha' })).not.toBeInTheDocument();
});

it('opens a choice card from its own control without choosing', async () => {
  const chosen = vi.fn(); const user = userEvent.setup();
  show(<Select label="Provider" value="b" onValueChange={chosen} options={[
    { value: 'a', name: 'Alpha', description: <span data-resource-hint="provider">62% used</span>, preview: { label: 'Alpha', content: () => <span>Provider details</span> } },
    { value: 'b', name: 'Bravo' },
  ]} />);
  await user.click(screen.getByRole('combobox', { name: 'Provider' }));
  // The control is clicked without moving a pointer onto it: a pointer at rest on it opens its name after a delay of the clock, and a pause
  // of the machine between the click and the Escape below would let the name open first and take the Escape.
  fireEvent.click(screen.getByRole('button', { name: 'Inspect Alpha' }));
  expect(await screen.findByRole('dialog', { name: 'Alpha' })).toHaveTextContent('Provider details');
  expect(chosen).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  expect(screen.getByRole('option', { name: 'Alpha' })).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith('a');
});

it('keeps the next option reachable when a phone picker has no room beside it', async () => {
  const previousFont = document.documentElement.style.fontSize;
  const previousWidth = window.innerWidth;
  const original = Reflect.get<HTMLElement, 'getBoundingClientRect'>(HTMLElement.prototype, 'getBoundingClientRect');
  const boxes = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect');
  try {
    document.documentElement.style.fontSize = '16px';
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 400 });
    boxes.mockImplementation(function (this: HTMLElement) {
      return this.getAttribute('role') === 'option'
        ? new DOMRect(20, 100, 360, 40) : original.call(this);
    });
    const chosen = vi.fn(), details = vi.fn(() => <span>Alpha details</span>), user = userEvent.setup();
    show(<Select label="Template" value="a" onValueChange={chosen} options={[
      { value: 'a', name: 'Alpha', preview: { label: 'Alpha', content: details } },
      { value: 'b', name: 'Bravo' },
    ]} />);
    await user.click(screen.getByRole('combobox', { name: 'Template' }));
    await waitFor(() => expect(screen.getByRole('option', { name: 'Alpha' })).toHaveFocus());
    expect(screen.queryByText('Alpha details')).not.toBeInTheDocument();
    await user.click(screen.getByRole('option', { name: 'Bravo' }));
    expect(chosen).toHaveBeenCalledExactlyOnceWith('b');
    await user.click(screen.getByRole('combobox', { name: 'Template' }));
    await user.click(screen.getByRole('button', { name: 'Inspect Alpha' }));
    expect(await screen.findByRole('dialog', { name: 'Alpha' })).toHaveTextContent('Alpha details');
    expect(chosen).toHaveBeenCalledTimes(1);
  } finally {
    boxes.mockRestore(); Object.defineProperty(window, 'innerWidth', { configurable: true, value: previousWidth }); document.documentElement.style.fontSize = previousFont;
  }
});

it('does not read details when focus is only passing through an option being chosen', async () => {
  const chosen = vi.fn(), details = vi.fn(() => <span>Alpha details</span>), user = userEvent.setup();
  show(<Select label="Provider" value="b" onValueChange={chosen} options={[
    { value: 'a', name: 'Alpha', preview: { label: 'Alpha', content: details } },
    { value: 'b', name: 'Bravo' },
  ]} />);
  await user.click(screen.getByRole('combobox', { name: 'Provider' }));
  const option = screen.getByRole('option', { name: 'Alpha' });
  act(() => { option.focus(); fireEvent.click(option); });
  expect(chosen).toHaveBeenCalledExactlyOnceWith('a');
  expect(details).not.toHaveBeenCalled();
});

it('keeps a linked name navigable while its card opens on focus', async () => {
  const followed = vi.fn(); const user = userEvent.setup();
  show(<DetailCard label="Provider" asChild content={() => <span>Provider details</span>}><a href="/resources/providers/provider" onClick={(event) => { followed(event.defaultPrevented); event.preventDefault(); }}>Provider</a></DetailCard>);
  await user.tab();
  expect(await screen.findByText('Provider details')).toBeVisible();
  await user.keyboard('{Enter}');
  expect(followed).toHaveBeenCalledExactlyOnceWith(false);
  expect(screen.queryByRole('dialog', { name: 'Provider' })).not.toBeInTheDocument();
});

it('follows a combobox virtual highlight and keeps its input and selection while inspecting', async () => {
  const choose = vi.fn(); const user = userEvent.setup();
  show(<Combobox label="Provider" query="" onQueryChange={() => undefined} onSelect={choose} groups={[{ id: 'p', label: '', options: [
    { value: 'a', label: 'Alpha', text: 'Alpha', preview: { label: 'Alpha', content: () => <button type="button">Inspect Alpha</button> } },
    { value: 'b', label: 'Bravo', text: 'Bravo', preview: { label: 'Bravo', content: () => <button type="button">Inspect Bravo</button> } },
  ] }]} />);
  const input = screen.getByRole('combobox', { name: 'Provider' });
  await user.click(input);
  const card = await screen.findByLabelText('Alpha', { selector: '[data-inspection-card]' });
  expect(within(card).getByRole('button', { name: 'Inspect Alpha' })).toBeVisible();
  await user.keyboard('{ArrowDown}{ArrowRight}');
  expect(await screen.findByRole('dialog', { name: 'Bravo' })).toBeVisible();
  expect(choose).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(input).toHaveFocus());
  expect(input).toHaveAttribute('aria-expanded', 'true');
  await user.keyboard('{ArrowUp}');
  expect(await screen.findByLabelText('Alpha', { selector: '[data-inspection-card]' })).toBeVisible();
  await user.keyboard('{ArrowDown}');
  expect(await screen.findByLabelText('Bravo', { selector: '[data-inspection-card]' })).toBeVisible();
  await act(async () => { await user.keyboard('{Enter}'); });
  expect(choose).toHaveBeenCalledWith(expect.objectContaining({ value: 'b' }));
});

it('keeps a menu and its selection while using an inspection action', async () => {
  const chosen = vi.fn(); const inspect = vi.fn(); const user = userEvent.setup();
  show(<Menu label="Models" trigger={<button type="button">Models</button>} entries={[{ type: 'radio', id: 'models', label: 'Model', value: 'a', onValueChange: chosen,
    options: [{ value: 'a', name: 'Alpha', preview: { label: 'Alpha', content: () => <button type="button" onClick={inspect}>Inspect model</button> } }] }]} />);
  await user.click(screen.getByRole('button', { name: 'Models' }));
  await user.keyboard('{ArrowDown}{ArrowRight}');
  expect(await screen.findByRole('dialog', { name: 'Alpha' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Inspect model' }));
  expect(inspect).toHaveBeenCalledOnce();
  expect(chosen).not.toHaveBeenCalled();
  expect(screen.getByRole('menu', { name: 'Models' })).toBeVisible();
  await user.keyboard('{Escape}');
  expect(screen.getByRole('menuitemradio', { name: 'Alpha' })).toHaveFocus();
});

it('keeps touch browsing clear until the option inspection control is used', async () => {
  const choose = vi.fn(), details = vi.fn(() => <span>Bravo details</span>), user = userEvent.setup();
  show(<Combobox label="Provider" query="" onQueryChange={() => undefined} onSelect={choose} groups={[{ id: 'p', label: '', options: [
    { value: 'a', label: 'Alpha', text: 'Alpha', preview: { label: 'Alpha', content: details } },
    { value: 'b', label: 'Bravo', text: 'Bravo', preview: { label: 'Bravo', content: details } },
  ] }]} />);
  const input = screen.getByRole('combobox', { name: 'Provider' });
  await user.pointer({ keys: '[TouchA]', target: input });
  expect(input).toHaveAttribute('aria-expanded', 'true');
  expect(details).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Inspect Bravo' }));
  expect(await screen.findByRole('dialog', { name: 'Bravo' })).toHaveTextContent('Bravo details');
  expect(choose).not.toHaveBeenCalled();
});

it('gives the first Escape to the form above a card that opened while the form covered it', () => {
  vi.useFakeTimers();
  const closed = vi.fn();
  const view = show(<>
    <DetailCard label="Workspace" content={() => <span>Workspace details</span>}>Workspace</DetailCard>
    <Dialog open title="Find a file" onOpenChange={closed}><input aria-label="Find" /></Dialog>
  </>);
  try {
    const trigger = screen.getByRole('button', { name: 'Workspace', hidden: true });
    // The pointer moved onto the control under the form, and its card opens hidden behind the form.
    fireEvent.pointerOver(trigger, { pointerType: 'mouse' });
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.runOnlyPendingTimers(); });
    // Without a way to be seen it is not drawn at all, so it cannot hold the key.
    expect(screen.queryByText('Workspace details')).not.toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(closed).toHaveBeenCalledWith(false);
  } finally { view.unmount(); vi.useRealTimers(); }
});

it('opens no card for a trigger that arrives under a pointer that did not move, and opens it when the pointer then moves on it', () => {
  vi.useFakeTimers();
  const view = show(<DetailCard label="Provider" content={() => <span>Provider details</span>}>Provider</DetailCard>);
  try {
    const trigger = screen.getByRole('button', { name: 'Provider' });
    // A dialog closes, a list scrolls or a bar opens under the pointer: the page is told of an entering and of no move.
    fireEvent.pointerOver(trigger, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS * 4); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
    // The person moves the pointer on it: the card follows after the usual rest, and not before it.
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS - 1); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(1); });
    expect(screen.getByText('Provider details')).toBeVisible();
    // Leaving to anything but the card closes it after the short grace.
    fireEvent.pointerOut(trigger, { pointerType: 'mouse', relatedTarget: document.body });
    act(() => { vi.advanceTimersByTime(1_000); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
  } finally { view.unmount(); vi.useRealTimers(); }
});

it('opens a card for a pointer that moves onto the trigger, and not when the pointer leaves before it rests', () => {
  vi.useFakeTimers();
  const view = show(<DetailCard label="Provider" content={() => <span>Provider details</span>}>Provider</DetailCard>);
  try {
    const trigger = screen.getByRole('button', { name: 'Provider' });
    fireEvent.pointerOver(trigger, { pointerType: 'mouse' });
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS - 1); });
    fireEvent.pointerOut(trigger, { pointerType: 'mouse', relatedTarget: document.body });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS * 2); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
    fireEvent.pointerOver(trigger, { pointerType: 'mouse', relatedTarget: document.body });
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS); });
    expect(screen.getByText('Provider details')).toBeVisible();
  } finally { view.unmount(); vi.useRealTimers(); }
});

it('keeps a touch from opening a card by pointer, and keyboard focus opens it as before', () => {
  vi.useFakeTimers();
  const view = show(<DetailCard label="Provider" content={() => <span>Provider details</span>}>Provider</DetailCard>);
  try {
    const trigger = screen.getByRole('button', { name: 'Provider' });
    fireEvent.pointerOver(trigger, { pointerType: 'touch' });
    fireEvent.pointerMove(trigger, { pointerType: 'touch' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS * 3); });
    expect(screen.queryByText('Provider details')).not.toBeInTheDocument();
    act(() => { trigger.focus(); });
    expect(screen.getByText('Provider details')).toBeVisible();
  } finally { view.unmount(); vi.useRealTimers(); }
});

it('opens a card of a people list by the same rule: a move onto it and a rest, never an arrival under a still pointer', () => {
  vi.useFakeTimers();
  const view = show(<IntentCard label="People" trigger={<button type="button">3 people</button>}><span>Ada, Grace and Linus</span></IntentCard>);
  try {
    const trigger = screen.getByRole('button', { name: '3 people' });
    fireEvent.pointerOver(trigger, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS * 4); });
    expect(screen.queryByText('Ada, Grace and Linus')).not.toBeInTheDocument();
    fireEvent.pointerMove(trigger, { pointerType: 'mouse' });
    act(() => { vi.advanceTimersByTime(CARD_OPEN_DELAY_MS); });
    expect(screen.getByText('Ada, Grace and Linus')).toBeVisible();
    fireEvent.pointerOut(trigger, { pointerType: 'mouse', relatedTarget: document.body });
    act(() => { vi.advanceTimersByTime(1_000); });
    expect(screen.queryByText('Ada, Grace and Linus')).not.toBeInTheDocument();
  } finally { view.unmount(); vi.useRealTimers(); }
});
