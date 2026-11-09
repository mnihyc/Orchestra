/** A focused search field explains its threshold without opening a reader or exposing stale choices. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { Combobox } from '@/ui/Combobox';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });
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
