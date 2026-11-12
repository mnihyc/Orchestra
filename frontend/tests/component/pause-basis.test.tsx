/** A pause or a resume is written on the state the dialog shows; a choice the person typed keeps the state it was typed on, and a refusal is told. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Away from '@/api/away';
import { awayPrefsQuery } from '@/api/away';
import { ApiError } from '@/api/client';
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
const before = useAppearance.getState();
const paused: AwayPrefs = { revision: 5, pause_until: '2099-01-01T14:00:00Z', approvals_always: true, chat_node_level: 'mentions', chat_direct_level: 'all', quiet_hours: null };
const later: AwayPrefs = { ...paused, revision: 6, pause_until: '2099-01-01T12:30:00Z' };
const stale = new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Your notification choices changed', detail: 'Your notification choices changed', status: 409, request_id: '' }, null);

beforeEach(() => {
  vi.clearAllMocks();
  useAppearance.setState({ locale: 'en', timeZone: 'UTC' });
});
afterEach(async () => { cleanup(); useAppearance.setState(before); await setLocale('en'); });

function open(prefs: AwayPrefs) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(awayPrefsQuery.queryKey, prefs);
  const view = show(<QueryClientProvider client={cache}><PauseControls /></QueryClientProvider>);
  return { cache, view };
}

it('writes a resume on the state the dialog shows when the preferences are read again while it is open', async () => {
  const { cache } = open(paused);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('away.pause.resume') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('away.pause.title') });
  expect(within(dialog).getByRole('status')).toHaveTextContent(/2:00\sPM/);
  // The read that was late answers: the dialog says the new pause, and what it writes is on that state.
  await act(async () => { cache.setQueryData(awayPrefsQuery.queryKey, later); await Promise.resolve(); });
  await vi.waitFor(() => expect(within(dialog).getByRole('status')).toHaveTextContent(/12:30\sPM/));
  calls.change.mockResolvedValueOnce({ ...later, revision: 7, pause_until: null });
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('away.pause.resume') }));
  expect(calls.change).toHaveBeenCalledWith({ expected_revision: 6, pause_until: null });
});

it('keeps the state a pause was typed on, so that a change made elsewhere meanwhile is refused and told', async () => {
  const { cache } = open({ ...paused, pause_until: null });
  await userEvent.click(screen.getByRole('button', { name: i18n.t('away.pause.button') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('away.pause.title') });
  await userEvent.type(within(dialog).getByRole('textbox', { name: i18n.t('away.pause.date') }), '2099-01-02');
  fireEvent.change(within(dialog).getByLabelText(i18n.t('away.pause.time')), { target: { value: '09:00' } });
  await act(async () => { cache.setQueryData(awayPrefsQuery.queryKey, later); await Promise.resolve(); });
  calls.change.mockRejectedValueOnce(stale);
  calls.read.mockResolvedValue({ ...later, revision: 7 });
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('away.pause.confirm') }));
  expect(calls.change).toHaveBeenCalledWith({ expected_revision: 5, pause_until: '2099-01-02T09:00:00.000Z' });
  expect(await within(dialog).findByText(i18n.t('away.pause.stale'))).toBeVisible();
});

it('tells a resume that the server refused as stale, shows how things stand and offers it again on that state', async () => {
  open(paused);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('away.pause.resume') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('away.pause.title') });
  calls.change.mockRejectedValueOnce(stale);
  calls.read.mockResolvedValue({ ...later, revision: 7 });
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('away.pause.resume') }));
  expect(await within(dialog).findByText(i18n.t('away.pause.stale'))).toBeVisible();
  expect(within(dialog).getByRole('status')).toHaveTextContent(/12:30\sPM/);
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('away.pause.useCurrent') }));
  calls.change.mockResolvedValueOnce({ ...later, revision: 8, pause_until: null });
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('away.pause.resume') }));
  expect(calls.change).toHaveBeenLastCalledWith({ expected_revision: 7, pause_until: null });
});
