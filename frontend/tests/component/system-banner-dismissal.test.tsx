/** A notice's complete server identity keeps its own dismissal on this device. */
import type * as Router from '@tanstack/react-router';
import type * as Live from '@/realtime/live';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { meQuery, metaQuery } from '@/api/auth';
import { client } from '@/api/client';
import type { MetaBanner } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { forgetViewChoiceDeviceData } from '@/lib/viewChoices';
import { Banners } from '@/shell/Banners';
import { me, meta } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useRouterState: () => '/settings/profile' }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useSocketStatus: () => ({ phase: 'ready', lostAt: null, outdated: false, roundTripMs: 20 }) }));
const first: MetaBanner = { level: 'info', text: 'The service will restart after work', until: null };
const reader = me(1, 'ada', 'Ada'), other = me(2, 'bo', 'Bo');
const session = client.session.getState();
let cache: QueryClient;
beforeEach(() => {
  cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  cache.setQueryData(metaQuery.queryKey, { ...meta(), banner: first });
  cache.setQueryData(meQuery.queryKey, reader);
  client.session.setState({ userId: reader.id });
});
afterEach(async () => {
  cleanup(); cache.clear(); localStorage.clear(); forgetViewChoiceDeviceData(reader.id, null); forgetViewChoiceDeviceData(other.id, null);
  client.session.setState(session); await setLocale('en');
});
const draw = () => show(<QueryClientProvider client={cache}><Banners /></QueryClientProvider>);

for (const locale of ['en', 'zh-CN'] as const) {
  it('remembers each dismissed notice separately after remount in '+locale, async () => {
    await setLocale(locale);
    const user = userEvent.setup(), view = draw();
    await user.click(screen.getByRole('button', { name: i18n.t('common.dismiss') }));
    expect(screen.queryByText(first.text)).not.toBeInTheDocument();
    expect(localStorage.length).toBe(1);
    view.unmount();
    const again = draw();
    expect(screen.queryByText(first.text)).not.toBeInTheDocument();
    const second = { ...first, text: 'The restart is tomorrow' };
    act(() => { cache.setQueryData(metaQuery.queryKey, { ...meta(), banner: second }); });
    expect(await screen.findByText(second.text)).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('common.dismiss') }));
    act(() => { cache.setQueryData(metaQuery.queryKey, { ...meta(), banner: first }); });
    again.unmount(); draw();
    await waitFor(() => expect(screen.queryByText(first.text)).not.toBeInTheDocument());
  });

  it.each(['level', 'until'] as const)('shows a notice with a changed %s in '+locale, async (field) => {
    await setLocale(locale); draw();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.dismiss') }));
    const next: MetaBanner = field === 'level' ? { ...first, level: 'warning' } : { ...first, until: '2026-12-01T00:00:00Z' };
    act(() => { cache.setQueryData(metaQuery.queryKey, { ...meta(), banner: next }); });
    expect(await screen.findByText(next.text)).toBeVisible();
  });

  it('does not apply one reader’s device dismissal to another reader in '+locale, async () => {
    await setLocale(locale); draw();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.dismiss') }));
    act(() => { client.session.setState({ userId: other.id }); });
    expect(screen.getByText(first.text)).toBeVisible();
  });
}
