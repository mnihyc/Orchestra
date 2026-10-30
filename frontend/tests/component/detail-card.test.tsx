/** Inspection follows hover and keyboard highlight; keeping and dismissing it never chooses a value. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';

import { DetailCard } from '@/ui/DetailCard';
import { Combobox } from '@/ui/Combobox';
import { Select } from '@/ui/Select';
import { Menu } from '@/ui/Menu';
import { show } from './support';

it('reads nothing before hover and closes a passing card after leaving', async () => {
  const content = vi.fn(() => <span>Provider details</span>);
  const user = userEvent.setup();
  show(<DetailCard label="Provider" content={content}>Provider</DetailCard>);
  expect(content).not.toHaveBeenCalled();
  const trigger = screen.getByRole('button', { name: 'Provider' });
  await user.hover(trigger);
  expect(await screen.findByText('Provider details')).toBeVisible();
  await user.unhover(trigger);
  await waitFor(() => expect(screen.queryByText('Provider details')).not.toBeInTheDocument());
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
  expect(await screen.findByRole('button', { name: 'Alpha details' })).toBeVisible();
  await user.keyboard('{ArrowDown}');
  expect(await screen.findByRole('button', { name: 'Bravo details' })).toBeVisible();
  expect(changed).not.toHaveBeenCalled();
  await user.keyboard('{ArrowRight}');
  expect(await screen.findByRole('dialog', { name: 'Bravo' })).toBeVisible();
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
  await user.click(screen.getByRole('button', { name: 'Inspect Alpha' }));
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
  const original = Reflect.get(HTMLElement.prototype, 'getBoundingClientRect') as HTMLElement['getBoundingClientRect'];
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
