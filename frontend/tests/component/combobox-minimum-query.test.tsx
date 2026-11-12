/** A focused search field explains its threshold without opening a reader or exposing stale choices. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { Combobox } from '@/ui/Combobox';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const rich of [false, true]) it(`marks pending choices unavailable and accepts the first ready selection (${rich ? 'rich' : 'plain'})`, async () => {
  const selected = vi.fn();
  const choice = { value: 'system', label: 'System', text: 'System' };
  const field = (loading: boolean) => <Combobox label="Scope" query="" onQueryChange={vi.fn()}
    loading={loading} onSelect={selected} groups={[{ id: 'scopes', label: '', options: [choice] }]}
    footer={rich ? <button type="button">More scopes</button> : undefined} />;
  const view = show(field(true));
  const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: 'Scope' }));
  expect(screen.getByRole('listbox', { name: 'Scope' })).toHaveAttribute('aria-busy', 'true');
  expect(screen.getByRole('option', { name: 'System' })).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByRole('option', { name: 'System' }));
  await user.keyboard('{Enter}');
  expect(selected).not.toHaveBeenCalled();
  view.rerender(field(false));
  expect(screen.getByRole('listbox', { name: 'Scope' })).not.toHaveAttribute('aria-busy');
  expect(screen.getByRole('option', { name: 'System' })).not.toHaveAttribute('aria-disabled');
  await user.click(screen.getByRole('option', { name: 'System' }));
  expect(selected).toHaveBeenCalledExactlyOnceWith(choice);
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`opens the search instruction before the minimum query without lending cached choices (${locale})`, async () => {
  await setLocale(locale);
  const read = vi.fn(), selected = vi.fn();
  function Field() {
    const [query, setQuery] = useState('');
    return <Combobox label="Recipient" minChars={2} query={query} onQueryChange={setQuery}
      onOpenChange={read} onSelect={selected} empty="No matching recipients"
      groups={[{ id: 'recipients', label: '', options: [{ value: 'recipient', label: 'Admitted recipient', text: 'Admitted recipient' }] }]} />;
  }
  show(<Field />); const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: 'Recipient' }));
  const list = screen.getByRole('listbox', { name: 'Recipient' });
  expect(within(list).getByText(i18n.t('ui.combobox.minimumQuery', { count: 2 }))).toBeVisible();
  expect(within(list).queryByRole('option')).toBeNull(); expect(read).not.toHaveBeenCalledWith(true);
  await user.type(screen.getByRole('combobox'), 'A'); await user.keyboard('{Enter}');
  expect(selected).not.toHaveBeenCalled(); expect(read).not.toHaveBeenCalledWith(true);
  await user.type(screen.getByRole('combobox'), 'd');
  await waitFor(() => expect(read).toHaveBeenCalledWith(true));
  await user.click(screen.getByRole('option', { name: 'Admitted recipient' }));
  expect(selected).toHaveBeenCalledExactlyOnceWith({ value: 'recipient', label: 'Admitted recipient', text: 'Admitted recipient' });
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`owns the actual plain popup as a listbox while selection keeps the input keyboard (${locale})`, async () => {
    await setLocale(locale);
    const selected = vi.fn();
    const choices = [
      { value: 'ada', label: 'Ada Lovelace', text: 'Ada Lovelace' },
      { value: 'grace', label: 'Grace Hopper', text: 'Grace Hopper' },
    ];
    function Picker() {
      const [query, setQuery] = useState('');
      return <Combobox label="Recipient" query={query} onQueryChange={setQuery} onSelect={selected}
        groups={[{ id: 'people', label: '', options: choices }]} />;
    }
    show(<Picker />);
    const user = userEvent.setup(), input = screen.getByRole('combobox', { name: 'Recipient' });
    await user.click(input);
    const popup = screen.getByRole('listbox', { name: 'Recipient' });
    expect(popup.id).toBe(input.getAttribute('aria-controls'));
    expect(input).toHaveAttribute('aria-haspopup', 'listbox');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await user.keyboard('{End}');
    expect(input).toHaveFocus();
    expect(screen.getByRole('option', { name: 'Grace Hopper', selected: true }).id).toBe(input.getAttribute('aria-activedescendant'));
    await user.keyboard('{Enter}');
    expect(selected).toHaveBeenCalledExactlyOnceWith(choices[1]);
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it(`keeps a rich popup's dialog ownership and paging keyboard path (${locale})`, async () => {
    await setLocale(locale);
    function Picker() {
      const [query, setQuery] = useState('');
      return <Combobox label="Recipient" query={query} onQueryChange={setQuery} onSelect={vi.fn()}
        groups={[{ id: 'people', label: '', options: [{ value: 'ada', label: 'Ada Lovelace', text: 'Ada Lovelace' }] }]}
        footer={<button type="button">More people</button>} />;
    }
    show(<Picker />);
    const user = userEvent.setup(), input = screen.getByRole('combobox', { name: 'Recipient' });
    await user.click(input);
    const popup = screen.getByRole('dialog', { name: 'Recipient' });
    expect(popup.id).toBe(input.getAttribute('aria-controls'));
    expect(input).toHaveAttribute('aria-haspopup', 'dialog');
    expect(within(popup).getByRole('listbox', { name: 'Recipient' })).toBeVisible();
    await user.keyboard('{Tab}');
    expect(within(popup).getByRole('button', { name: 'More people' })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(input).toHaveFocus();
  });
}
