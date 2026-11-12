/** Group controls stay operable beside valid option lists and keep one keyboard selection order. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const grouped of [false, true]) it(`owns only options while keeping row actions operable in ${locale}/${grouped ? 'groups' : 'plain'}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup(), chosen = vi.fn(), inspect = vi.fn();
  const field = i18n.t('settingRegistry.agent.default_provider_id.title');
  function Rows() {
    const [query, setQuery] = useState('');
    const option = (value: string): ComboboxOption<string> => ({ value, label: `${value} provider`, text: `${value} provider`,
      accessory: <button onClick={inspect}>Inspect {value}</button> });
    const alpha = option('Alpha'), beta = option('Beta');
    return <Combobox label={field} minChars={0} query={query} onQueryChange={setQuery} onSelect={chosen} groups={grouped
      ? [{ id: 'alpha', label: 'Alpha', heading: <button onClick={inspect}>Review Alpha</button>, options: [alpha] }, { id: 'beta', label: 'Beta', options: [beta] }]
      : [{ id: 'all', label: '', options: [alpha, beta] }]} />;
  }
  show(<Rows />);
  const input = screen.getByRole('combobox', { name: field });
  await user.click(input);
  const lists = screen.getAllByRole('listbox');
  expect(lists).toHaveLength(grouped ? 2 : 1);
  const owned = lists.flatMap((list) => {
    expect(list.querySelector('button,a[href]')).toBeNull();
    expect(list).not.toHaveAttribute('aria-owns');
    const options = within(list).getAllByRole('option');
    for (const option of options) expect(option.closest('[role="listbox"]')).toBe(list);
    return options;
  });
  expect(owned).toEqual(screen.getAllByRole('option'));
  const action = screen.getByRole('button', { name: 'Inspect Alpha' });
  expect(action.closest('[role="listbox"]')).toBeNull();
  await user.click(action);
  expect(inspect).toHaveBeenCalledTimes(1); expect(chosen).not.toHaveBeenCalled();
  await user.click(input); await user.keyboard('{End}{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ value: 'Beta' }));
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps group actions outside option ownership in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup(), chosen = vi.fn(), review = vi.fn();
  const field = i18n.t('settingRegistry.agent.default_provider_id.title');
  function Grouped() {
    const [folded, setFolded] = useState(false), [query, setQuery] = useState('');
    const option = (value: string): ComboboxOption<string> => ({ value, label: `${value} provider`, text: `${value} provider` });
    return <Combobox label={field} minChars={0} query={query} onQueryChange={setQuery} onSelect={chosen} groups={[
      { id: 'alpha', label: 'Alpha', collapsed: folded, heading: <>
        <button onClick={() => setFolded((before) => !before)}>{folded ? 'Expand Alpha' : 'Collapse Alpha'}</button>
        <a href="#alpha-home">Alpha home</a><button onClick={review}>Review Alpha sharing</button>
      </>, options: [option('Alpha')] },
      { id: 'beta', label: 'Beta', heading: <button onClick={review}>Review Beta sharing</button>, options: [option('Beta')] },
    ]} />;
  }
  show(<Grouped />);
  const input = screen.getByRole('combobox', { name: field });
  await user.click(input);
  const lists = screen.getAllByRole('listbox');
  expect(lists).toHaveLength(2);
  for (const list of lists) {
    expect(list.querySelector('button,a[href]')).toBeNull();
    expect(within(list).getAllByRole('option')).toHaveLength(1);
  }
  expect(screen.getByRole('listbox', { name: `${field} · Alpha` })).toBeVisible();
  expect(screen.getByRole('listbox', { name: `${field} · Beta` })).toBeVisible();
  expect(screen.getByRole('link', { name: 'Alpha home' }).closest('[role="listbox"]')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Review Alpha sharing' }));
  expect(review).toHaveBeenCalledTimes(1); expect(chosen).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Collapse Alpha' }));
  expect(screen.queryByRole('option', { name: 'Alpha provider' })).not.toBeInTheDocument();
  expect(screen.getByRole('option', { name: 'Beta provider' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Expand Alpha' }));
  expect(screen.getByRole('option', { name: 'Alpha provider' })).toBeVisible();
  expect(chosen).not.toHaveBeenCalled();
  await user.click(input);
  await user.keyboard('{End}{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ value: 'Beta' }));
});
