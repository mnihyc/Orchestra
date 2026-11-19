/** Complete choice declarations search locally and preserve committed values, guides and form boundaries. */
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { ChoiceLocator, type ChoiceLocatorOption } from '@/ui/ChoiceLocator';
import { Combobox } from '@/ui/Combobox';
import { show } from './support';

const options = Array.from({ length: 12 }, (_, index) => ({ value: `native-${index + 1}`, name: `Choice ${index + 1}`, description: `Description ${index + 1}` }));

it('searches every supplied name, raw value and description without editing or fetching, and restores an unknown current value', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const read = vi.spyOn(globalThis, 'fetch');
  function Example() {
    const [value, setValue] = useState('retired-native-value');
    return <ChoiceLocator label="Declared choice" value={value} valueLabel={value === 'retired-native-value' ? 'Retired choice' : undefined}
      options={options} onValueChange={(next) => { changed(next); setValue(next); }} />;
  }
  try {
    show(<Example />);
    const input = screen.getByRole('combobox', { name: 'Declared choice' });
    expect(input).toHaveValue('Retired choice');
    await user.click(input);
    expect(screen.getAllByRole('option')).toHaveLength(12);
    // One unnamed list: no empty group name and no empty heading row above the choices.
    expect(within(screen.getByRole('listbox')).queryByRole('group')).toBeNull();
    await user.type(input, 'DESCRIPTION 12');
    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option', { name: 'Choice 12' })).toHaveAccessibleDescription('Description 12');
    expect(changed).not.toHaveBeenCalled();
    expect(read).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(input).toHaveValue('Retired choice');
    await user.click(input);
    await user.type(input, 'native-12');
    await user.keyboard('{Enter}');
    expect(changed).toHaveBeenCalledExactlyOnceWith('native-12');
    expect(input).toHaveValue('Choice 12');
    expect(read).not.toHaveBeenCalled();
  } finally { read.mockRestore(); }
});

it('keeps empty raw values, field slots, full labels and icons, and opens on the committed choice', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const supplied: ChoiceLocatorOption<string>[] = [{ value: '', name: 'Automatic' }, ...options,
    { value: 'long', name: 'Short name', fullLabel: 'The complete declared name', description: 'A whole explanation.', icon: <span data-testid="choice-icon">◎</span> }];
  function Example() {
    const [value, setValue] = useState('native-12');
    return <ChoiceLocator id="declared-choice" label="Mode" value={value} options={supplied} optional hint="Field hint" error="Field problem"
      help={<button type="button">Field guide</button>} aside={<span>Field aside</span>} fieldClassName="field-frame" className="field-input"
      onValueChange={(next) => { changed(next); setValue(next); }} />;
  }
  show(<Example />);
  const input = screen.getByRole('combobox', { name: /^Mode/ });
  expect(input).toHaveAttribute('id', 'declared-choice');
  expect(input).toHaveAccessibleDescription('Field problem');
  expect(input).toHaveClass('field-input');
  expect(input.closest('.field-frame')).not.toBeNull();
  expect(screen.getByRole('button', { name: 'Field guide' })).toBeVisible();
  expect(screen.getByText('Field aside')).toBeVisible();
  await user.click(input);
  await waitFor(() => expect(input).toHaveAttribute('aria-activedescendant', screen.getByRole('option', { name: 'Choice 12' }).id));
  await user.keyboard('{Enter}');
  expect(changed).not.toHaveBeenCalled();
  await user.click(input);
  await user.type(input, 'complete declared');
  const full = screen.getByRole('option', { name: 'Short name' });
  expect(full.querySelector('[data-overflow-text]')).toHaveAttribute('data-overflow-text', 'The complete declared name');
  expect(within(full).getByTestId('choice-icon')).toBeVisible();
  await user.click(full);
  expect(input).toHaveValue('Short name');
  expect(input).toHaveAttribute('data-overflow-text', 'The complete declared name');
  await user.click(input);
  await user.click(screen.getByRole('option', { name: 'Automatic' }));
  expect(changed.mock.calls).toEqual([['long'], ['']]);
  expect(input).toHaveValue('Automatic');
});

it('reaches active and disabled guides outside the listbox by Tab, F1 and pointer without choosing', async () => {
  const user = userEvent.setup(), changed = vi.fn(), currentGuide = vi.fn(), blockedGuide = vi.fn();
  show(<ChoiceLocator label="Mode" value="native-12" onValueChange={changed} options={[
    ...options.map((option) => option.value === 'native-12' ? { ...option, help: { label: 'Guide to Choice 12', onOpen: currentGuide } } : option),
    { value: 'blocked', name: 'Unavailable choice', description: 'Ask the owner to enable it.', disabled: true,
      help: { label: 'Guide to unavailable choice', onOpen: blockedGuide } },
  ]} />);
  const input = screen.getByRole('combobox', { name: 'Mode' });
  await user.click(input);
  const blocked = screen.getByRole('option', { name: 'Unavailable choice' });
  expect(blocked).toHaveAccessibleDescription('Ask the owner to enable it.');
  expect(blocked).toHaveAttribute('aria-disabled', 'true');
  const popup = screen.getByRole('dialog', { name: 'Mode' });
  expect(input).toHaveAttribute('aria-controls', popup.id);
  const guide = screen.getByRole('button', { name: 'Guide to Choice 12' });
  expect(screen.getByRole('listbox', { name: 'Mode' })).not.toContainElement(guide);
  await user.tab();
  expect(guide).toHaveFocus();
  await user.tab({ shift: true });
  expect(input).toHaveFocus();
  await user.keyboard('{F1}');
  expect(currentGuide).toHaveBeenCalledOnce();
  expect(input).toHaveValue('Choice 12');
  await user.click(input);
  await user.type(input, 'unavailable');
  await user.keyboard('{Enter}');
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Guide to unavailable choice' }));
  expect(blockedGuide).toHaveBeenCalledOnce();
  expect(changed).not.toHaveBeenCalled();
  expect(input).toHaveValue('Choice 12');
});

it('does not submit or choose a previous match on empty, dismissed or composing Enter', async () => {
  const user = userEvent.setup(), changed = vi.fn(), submit = vi.fn();
  show(<form onSubmit={(event) => { event.preventDefault(); submit(); }}>
    <ChoiceLocator label="Mode" value="native-1" options={options} onValueChange={changed} /><button type="submit">Accept</button>
  </form>);
  const input = screen.getByRole('combobox', { name: 'Mode' });
  await user.click(input);
  await user.type(input, 'no matching choice');
  expect(screen.queryByRole('option')).not.toBeInTheDocument();
  await user.keyboard('{Enter}{Escape}{Enter}');
  expect(input).toHaveValue('Choice 1');
  await user.click(input);
  await user.type(input, 'native-12');
  fireEvent.keyDown(input, { key: 'Enter', code: 'Enter', isComposing: true, keyCode: 229 });
  expect(changed).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  expect(changed).toHaveBeenCalledExactlyOnceWith('native-12');
  expect(submit).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Accept' }));
  expect(submit).toHaveBeenCalledOnce();
});

it('retains the ordinary lazy Combobox query, open notifications and string-disabled reasons', async () => {
  const user = userEvent.setup(), selected = vi.fn(), opened = vi.fn(), typed = vi.fn();
  function ExistingCaller() {
    const [query, setQuery] = useState('');
    return <Combobox label="Existing search" query={query} minChars={2} onQueryChange={(next) => { typed(next); setQuery(next); }}
      onOpenChange={opened} onSelect={selected} groups={[{ id: 'people', label: 'People', options: [
        { value: 'unavailable', label: 'Unavailable', text: 'Unavailable', disabled: 'Ask the owner.' },
        { value: 'ready', label: 'Ready', text: 'Ready', description: 'Can be used.' },
      ] }]} />;
  }
  show(<ExistingCaller />);
  const input = screen.getByRole('combobox', { name: 'Existing search' });
  await user.click(input); await user.type(input, 'a');
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  await user.type(input, 'b');
  expect(opened).toHaveBeenLastCalledWith(true);
  expect(typed).toHaveBeenLastCalledWith('ab');
  expect(screen.getByRole('option', { name: 'Unavailable' })).toHaveAccessibleDescription('Ask the owner.');
  await user.keyboard('{Enter}'); expect(selected).not.toHaveBeenCalled();
  await user.keyboard('{ArrowDown}{Enter}');
  expect(selected).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ value: 'ready' }));
  expect(opened).toHaveBeenLastCalledWith(false);
});
