/** A person finds, changes, removes and restores a binding without losing other state or refreshing the page. */
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { useEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { KeyList, keyActionName } from '@/features/help/KeyList';
import { meStateKey, meStateQuery } from '@/api/personal';
import { i18n, setLocale } from '@/i18n';
import { keyChoicesOf, SHIPPED_CHOICES } from '@/shell/keyActions';
import { actionKeys, bindAction, keyBook, setKeyChoices } from '@/shell/keys';
import { TooltipProvider } from '@/ui/Tooltip';
import { ShortcutDialog } from '@/shell/Shortcuts';

const calls = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn((_key: string, _value: unknown) => Promise.resolve()) }));
vi.mock('@/api/personal', async (original) => ({ ...await original<object>(),
  meStateQuery: { queryKey: ['me-state'], queryFn: () => { calls.read(); return Promise.resolve({}); } },
  setMeState: (key: string, value: unknown) => calls.write(key, value),
}));
let unbind: (() => void) | undefined;
let originalTheme: string | null;
beforeEach(() => { originalTheme = document.documentElement.getAttribute('data-theme'); calls.read.mockClear(); calls.write.mockReset(); calls.write.mockImplementation(() => Promise.resolve()); setKeyChoices(SHIPPED_CHOICES); unbind = bindAction('inbox', () => true); });
afterEach(async () => { cleanup(); unbind?.(); setKeyChoices(SHIPPED_CHOICES); if (originalTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.dataset.theme = originalTheme; await setLocale('en'); });

function draw(dialog = false) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Number.POSITIVE_INFINITY }, mutations: { retry: false } } });
  cache.setQueryData(meStateKey, { other: { untouched: true }, keys: { extra: 'kept', single_key: true, bindings: { 'future-action': 'Alt+9' } } });
  // The actual shell mirrors personal state into the one book, even when the help tab draws the list.
  function Kept() {
    const state = useQuery(meStateQuery).data;
    useEffect(() => { setKeyChoices(keyChoicesOf(state?.keys)); }, [state]);
    return dialog ? <ShortcutDialog open onOpenChange={() => undefined} /> : <KeyList />;
  }
  render(<QueryClientProvider client={cache}><TooltipProvider><Kept /></TooltipProvider></QueryClientProvider>);
  return cache;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`opens the shell's searchable shortcut table and narrows by its action at ${locale}/${theme}`, async () => {
  await setLocale(locale); document.documentElement.dataset.theme = theme;
  draw(true); const user = userEvent.setup();
  const dialog = screen.getByRole('dialog', { name: i18n.t('shell.shortcuts.title') });
  const search = within(dialog).getByRole('searchbox', { name: i18n.t('shell.keyChoice.search') });
  expect(within(dialog).getByRole('table', { name: i18n.t('shell.keyChoice.table') })).toBeVisible();
  await user.type(search, keyActionName(i18n.t, 'inbox'));
  const table = within(dialog).getByRole('table', { name: i18n.t('shell.keyChoice.table') });
  expect(within(table).getAllByRole('row')).toHaveLength(2);
  expect(within(table).getByText(keyActionName(i18n.t, 'inbox'))).toBeVisible();
  expect(calls.write).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`finds and edits a binding from the keyboard (${locale}, ${theme})`, async () => {
  await setLocale(locale); document.documentElement.dataset.theme = theme;
  const t = i18n.getFixedT(locale); const cache = draw(); const user = userEvent.setup();
  const search = screen.getByRole('searchbox', { name: t('shell.keyChoice.search') });
  await user.click(search); await user.type(search, locale === 'en' ? 'inbox' : '收件箱');
  const table = screen.getByRole('table', { name: t('shell.keyChoice.table') });
  expect(within(table).getAllByRole('row')).toHaveLength(2);
  const name = keyActionName(t, 'inbox');
  const change = within(table).getByRole('button', { name: t('shell.keyChoice.changeOf', { action: name }) });
  // A named table scroller is itself a keyboard destination before its row controls.
  const scroller = screen.getByRole('region', { name: t('shell.keyChoice.table') });
  expect(scroller).toContainElement(table);
  expect(scroller).toHaveAttribute('tabindex', '0');
  await user.tab(); expect(scroller).toHaveFocus();
  await user.tab(); expect(change).toHaveFocus(); await user.keyboard('{Enter}');
  expect(screen.getByRole('group', { name: t('shell.keyChoice.pressing', { action: name }) })).toHaveFocus();
  await user.keyboard('gn');
  await waitFor(() => expect(actionKeys('inbox')).toBe('G N'));
  expect(within(table).getByRole('button', { name: t('shell.keyChoice.changeOf', { action: name }) })).toHaveFocus();
  expect(calls.write).toHaveBeenCalledExactlyOnceWith('keys', { extra: 'kept', single_key: true, bindings: { 'future-action': 'Alt+9', inbox: 'G N' } });
  expect(calls.read).not.toHaveBeenCalled();
  expect(cache.getQueryData(meStateKey)).toMatchObject({ other: { untouched: true } });

  await user.click(within(table).getByRole('button', { name: t('shell.keyChoice.removeOf', { action: name }) }));
  await waitFor(() => expect(actionKeys('inbox')).toBeUndefined());
  expect(within(table).getByText(t('shell.keyChoice.none'))).toBeVisible();
  await user.click(within(table).getByRole('button', { name: t('shell.keyChoice.resetOf', { action: name, keys: 'G I' }) }));
  await waitFor(() => expect(actionKeys('inbox')).toBe('G I'));
  expect(calls.write).toHaveBeenCalledTimes(3);
  expect(calls.read).not.toHaveBeenCalled();
  expect(cache.getQueryData(meStateKey)).toMatchObject({ keys: { bindings: { 'future-action': 'Alt+9' } } });
});

for (const locale of ['en', 'zh-CN'] as const) it(`clears an empty search back to the complete table and its search field without a request (${locale})`, async () => {
  await setLocale(locale); draw(); const user = userEvent.setup(); const t = i18n.getFixedT(locale);
  const search = screen.getByRole('searchbox', { name: t('shell.keyChoice.search') });
  await user.type(search, 'unmatched-shortcut-938847');
  expect(screen.getByText(t('shell.keyChoice.nothing', { query: 'unmatched-shortcut-938847' }))).toBeVisible();
  await user.click(screen.getByRole('button', { name: t('shell.keyChoice.clearSearch') }));
  expect(search).toHaveValue(''); expect(search).toHaveFocus();
  expect(screen.getByRole('table', { name: t('shell.keyChoice.table') })).toBeVisible();
  expect(calls.read).not.toHaveBeenCalled(); expect(calls.write).not.toHaveBeenCalled();
});

it('keeps the old keys after a failed save and retries the proposed binding without another state read', async () => {
  await setLocale('en'); draw(); const user = userEvent.setup();
  const search = screen.getByRole('searchbox', { name: 'Search keyboard shortcuts' }); await user.type(search, 'inbox');
  calls.write.mockRejectedValueOnce(new Error('Unavailable'));
  await user.click(screen.getByRole('button', { name: 'Change the keys of Go to the inbox' }));
  await user.keyboard('gn');
  await waitFor(() => expect(screen.getByText('Not saved: G N. The keys before still act.')).toBeVisible());
  expect(actionKeys('inbox')).toBe('G I');
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(actionKeys('inbox')).toBe('G N'));
  expect(calls.write).toHaveBeenCalledTimes(2); expect(calls.read).not.toHaveBeenCalled();
});

it('cancels a half-recorded sequence on blur and gives Escape back to Change, without a save', async () => {
  await setLocale('en'); draw(); const user = userEvent.setup();
  const search = screen.getByRole('searchbox', { name: 'Search keyboard shortcuts' }); await user.type(search, 'inbox');
  const change = () => screen.getByRole('button', { name: 'Change the keys of Go to the inbox' });
  await user.click(change()); await user.keyboard('g'); await user.click(search);
  expect(screen.queryByRole('group', { name: 'New keys for Go to the inbox' })).toBeNull();
  expect(search).toHaveFocus();
  await user.click(change()); await user.keyboard('{Escape}');
  expect(change()).toHaveFocus(); expect(calls.write).not.toHaveBeenCalled();
});

it('refuses a taken shortcut and leaves the native action bound to its former keys', async () => {
  await setLocale('en'); draw(); const user = userEvent.setup();
  const away = bindAction('finder', () => true);
  try {
    await user.type(screen.getByRole('searchbox', { name: 'Search keyboard shortcuts' }), 'inbox');
    await user.click(screen.getByRole('button', { name: 'Change the keys of Go to the inbox' }));
    // A one-chord conflict completes after the actual sequence window; no timeout is widened.
    await user.keyboard('{Control>}k{/Control}');
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('already does something else'), { timeout: 2000 });
    expect(actionKeys('inbox')).toBe('G I');
    act(() => {
      expect(keyBook.handle(new KeyboardEvent('keydown', { key: 'g', cancelable: true }))).toBe(true);
      expect(keyBook.handle(new KeyboardEvent('keydown', { key: 'i', cancelable: true }))).toBe(true);
    });
  } finally { away(); }
});
