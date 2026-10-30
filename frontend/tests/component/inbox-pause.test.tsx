/** UA-29: idle/expired pauses earn no banner; the active status and modal each say their value once. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Away from '@/api/away';
import { awayPrefsQuery } from '@/api/away';
import { PauseControls } from '@/features/settings/personal/away/Pause';
import type { AwayPrefs } from '@/features/settings/personal/away/types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<AwayPrefs>>(), change: vi.fn<typeof Away.changePrefs>() }));
vi.mock('@/api/away', async (original) => {
  const api = await original<typeof Away>();
  return { ...api, awayPrefsQuery: { ...api.awayPrefsQuery, queryFn: calls.read, staleTime: Infinity }, changePrefs: calls.change };
});
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const wasDark = document.documentElement.classList.contains('dark');
const previousTheme = document.documentElement.dataset.theme;
const base: AwayPrefs = { revision: 5, pause_until: null, approvals_always: true, chat_node_level: 'mentions', chat_direct_level: 'all', quiet_hours: null };
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

it.each(modes)('shows a compact idle action, active-only status and one confirmed resume in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme, locale, timeZone: 'UTC' });
  document.documentElement.classList.toggle('dark', theme === 'dark'); document.documentElement.dataset.theme = theme;
  for (const pause of [null, '2020-01-01T00:00:00Z', '2099-01-01T00:00:00Z']) {
    const prefs = { ...base, pause_until: pause };
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(awayPrefsQuery.queryKey, prefs);
    const view = show(<QueryClientProvider client={cache}><PauseControls /></QueryClientProvider>);
    const container = view.container.querySelector<HTMLElement>('[data-away-pause]');
    if (container === null) throw new Error('the one pause control is ready from the supplied cache');
    const controls = within(container);
    const active = pause?.startsWith('2099') === true;
    const label = i18n.t(active ? 'away.pause.resume' : 'away.pause.button');
    const action = controls.getByRole('button', { name: label });
    if (active) {
      expect(controls.getAllByRole('status')).toHaveLength(1);
      expect(controls.getByRole('button', { name: i18n.t('away.pause.change') })).toBeVisible();
    } else {
      expect(controls.queryByRole('status')).toBeNull();
      expect(container.querySelector('p')).toBeNull();
      expect(controls.queryByRole('button', { name: i18n.t('away.pause.resume') })).toBeNull();
    }
    expect(screen.queryByText(i18n.t('away.pause.boundary'))).toBeNull();
    await userEvent.click(action);
    const dialog = screen.getByRole('dialog', { name: i18n.t('away.pause.title') });
    expect(within(dialog).getAllByRole('status')).toHaveLength(1);
    expect(controls.queryByRole('status')).toBeNull();
    expect(within(dialog).getByText(i18n.t('away.pause.boundary'))).toBeVisible();
    if (active) {
      // The act registry applies this supplied response to this same key; no refresh is necessary.
      calls.change.mockImplementationOnce((body) => {
        const next = { ...prefs, revision: 6, pause_until: body.pause_until ?? null };
        cache.setQueryData(awayPrefsQuery.queryKey, next);
        return Promise.resolve(next);
      });
      expect(screen.getAllByRole('button', { name: i18n.t('away.pause.resume') })).toHaveLength(1);
      await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('away.pause.resume') }));
      await screen.findByRole('button', { name: i18n.t('away.pause.button') });
      expect(calls.change).toHaveBeenCalledWith({ expected_revision: 5, pause_until: null });
      expect(controls.queryByRole('status')).toBeNull();
    } else {
      expect(within(dialog).getByRole('status')).toHaveTextContent(i18n.t('away.pause.unpaused'));
      await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
      expect(action).toBeVisible();
    }
    view.unmount(); cache.clear();
  }
  expect(calls.change).toHaveBeenCalledTimes(1);
  expect(calls.read).not.toHaveBeenCalled();
});

it('removes the pause status at its exact local deadline without a preference read or write', async () => {
  vi.useFakeTimers();
  const at = Date.now();
  vi.setSystemTime(at);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(awayPrefsQuery.queryKey, { ...base, pause_until: new Date(at + 1_000).toISOString() });
  try {
    show(<QueryClientProvider client={cache}><PauseControls /></QueryClientProvider>);
    expect(screen.getByRole('status')).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('away.pause.button') })).toBeVisible();
    expect(calls.read).not.toHaveBeenCalled();
    expect(calls.change).not.toHaveBeenCalled();
  } finally { cleanup(); cache.clear(); vi.useRealTimers(); }
});
