/** A pause stays confirmed while saving, keeps proposed edits after a stale revision, and never implies receipt. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { changePrefs } from '@/api/away';
import type * as AwayApi from '@/api/away';
import { ApiError } from '@/api/client';
import { PauseControls, PauseDialog } from '@/features/settings/personal/away/Pause';
import { ConfiguredAway } from '@/features/settings/personal/away/Status';
import type { AwayPrefs } from '@/features/settings/personal/away/types';
import { useAppearance } from '@/lib/appearance';
import { i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';

vi.setConfig({ testTimeout: 30_000 });
vi.mock('@/api/away', async () => {
  const actual = await vi.importActual<typeof AwayApi>('@/api/away');
  return { ...actual, changePrefs: vi.fn(), awayPrefsQuery: { ...actual.awayPrefsQuery, queryFn: vi.fn(() => Promise.resolve({ revision: 8, pause_until: '2099-01-01T06:00:00Z', approvals_always: true, chat_node_level: 'mentions', chat_direct_level: 'all', quiet_hours: null })) } };
});
const prefs: AwayPrefs = { revision: 5, pause_until: null, approvals_always: true, chat_node_level: 'mentions', chat_direct_level: 'all', quiet_hours: { start_minute: 22 * 60, end_minute: 7 * 60 } };
const future = '2099-01-01';
function draw() {
  useAppearance.setState({ timeZone: 'Asia/Singapore', locale: 'en' });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const close = vi.fn<(open: boolean) => void>();
  render(<QueryClientProvider client={client}><TooltipProvider><PauseDialog open initial={prefs} mode="pause" onOpenChange={close} /></TooltipProvider></QueryClientProvider>);
  return { client, close };
}
afterEach(async () => { vi.clearAllMocks(); cleanup(); await setLocale('en'); });

describe('the shared pause form', () => {
  it.each(['en', 'zh-CN'] as const)('keeps precise pause limits in the deliberate dialog, not an idle Inbox paragraph (%s)', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><PauseControls /></QueryClientProvider>);
    const resume = await screen.findByRole('button', { name: t('away.pause.resume') });
    expect(screen.queryByText(t('away.pause.boundary'))).not.toBeInTheDocument();
    await userEvent.click(resume);
    expect(screen.getByText(t('away.pause.boundary'))).toBeVisible();
    expect(changePrefs).not.toHaveBeenCalled();
    cleanup();
    await setLocale('en');
  });
  it('sends only the explicit zoned instant and the displayed revision, keeping the confirmed state pending', async () => {
    const { close } = draw();
    let finish: (value: AwayPrefs) => void = () => undefined;
    vi.mocked(changePrefs).mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    expect(screen.getByLabelText('Date')).toHaveValue('');
    await userEvent.type(screen.getByLabelText('Date'), future);
    await userEvent.type(screen.getByLabelText('Time'), '12:30');
    await userEvent.click(screen.getByRole('button', { name: 'Pause notifications' }));
    expect(changePrefs).toHaveBeenCalledWith({ expected_revision: 5, pause_until: '2099-01-01T04:30:00.000Z' });
    expect(screen.getByText('Outside notifications are not paused')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Pause notifications' })).toHaveAttribute('aria-busy', 'true');
    expect(close).not.toHaveBeenCalled();
    finish({ ...prefs, revision: 6, pause_until: '2099-01-01T04:30:00.000Z' });
    await waitFor(() => expect(close).toHaveBeenCalledWith(false));
  });
  it('requires an explicit new choice after a stale revision and retains the proposed time', async () => {
    draw();
    vi.mocked(changePrefs).mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Changed', status: 409, detail: 'Changed', request_id: 'stale' }, null));
    await userEvent.type(screen.getByLabelText('Date'), future);
    await userEvent.type(screen.getByLabelText('Time'), '12:30');
    await userEvent.click(screen.getByRole('button', { name: 'Pause notifications' }));
    await screen.findByText('Your choices changed in another window');
    expect(screen.getByLabelText('Date')).toHaveValue(future);
    expect(screen.getByLabelText('Time')).toHaveValue('12:30');
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Keep my proposed change' }));
    vi.mocked(changePrefs).mockResolvedValueOnce({ ...prefs, revision: 9, pause_until: '2099-01-01T04:30:00Z' });
    await userEvent.click(screen.getByRole('button', { name: 'Pause notifications' }));
    expect(changePrefs).toHaveBeenLastCalledWith({ expected_revision: 8, pause_until: '2099-01-01T04:30:00.000Z' });
  });
  it('a configured first-work fact offers no permission prompt or automatic test', () => {
    const client = new QueryClient();
    render(<QueryClientProvider client={client}><ConfiguredAway configured={false} nodeId="org" /></QueryClientProvider>);
    expect(screen.getByText('Outside Orch: off')).toBeVisible();
    expect(changePrefs).not.toHaveBeenCalled();
  });
});
