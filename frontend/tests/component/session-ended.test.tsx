/** A real refused restore distinguishes a previous device session from a first entrance. */
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createApiClient } from '@/api/client';
import { SessionEndedNotice } from '@/features/auth/SessionEnded';
import type { AuthResult } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { me } from '../doubles/fixtures';
import { show } from './support';

const options = { baseUrl: 'http://localhost/api/session-notice', fetch: () => Promise.resolve(refused(401)) };
function refused(status: number): Response {
  return Response.json({ type: 'orch:error:auth.required', code: 'auth.required', title: 'Sign in', detail: 'Sign in', status, request_id: 'test' },
    { status, headers: { 'content-type': 'application/problem+json' } });
}
const grant = (): AuthResult => ({ access_token: 'test-access', access_expires_at: new Date(Date.now() + 900_000).toISOString(), user: me(1, 'reader', 'Reader') });
beforeEach(() => { localStorage.clear(); vi.useFakeTimers(); });
afterEach(async () => {
  cleanup(); vi.clearAllTimers(); vi.useRealTimers(); localStorage.clear();
  useAppearance.setState({ locale: 'en' }); await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`says an ended restored session and clears the words at a new sign-in in ${locale}`, async () => {
    useAppearance.setState({ locale }); await setLocale(locale);
    const previousPage = createApiClient(options);
    previousPage.adopt(grant());
    const current = createApiClient(options);
    show(<SessionEndedNotice session={current.session} />);
    expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
    await act(async () => { expect(await current.restore()).toBe(false); });
    expect(screen.getByText(i18n.t('auth.login.sessionEnded'))).toBeVisible();
    act(() => current.adopt(grant()));
    expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
  });
}

it('has no ended-session words for the first visit or a deliberate sign-out', async () => {
  const first = createApiClient(options);
  const view = show(<SessionEndedNotice session={first.session} />);
  await act(async () => { expect(await first.restore()).toBe(false); });
  expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
  act(() => first.adopt(grant()));
  await act(async () => { await first.signOut(); });
  expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
  const nextPage = createApiClient(options);
  view.rerender(<SessionEndedNotice session={nextPage.session} />);
  await act(async () => { expect(await nextPage.restore()).toBe(false); });
  expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
});

it('does not call a temporary network failure or a refused account an ended session', async () => {
  createApiClient(options).adopt(grant());
  const offline = createApiClient({ ...options, fetch: () => Promise.reject(new NetworkError('offline')) });
  const view = show(<SessionEndedNotice session={offline.session} />);
  await act(async () => { await expect(offline.restore()).rejects.toBeInstanceOf(NetworkError); });
  expect(offline.session.getState().status).toBe('unknown');
  expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
  const suspended = createApiClient({ ...options, fetch: () => Promise.resolve(refused(403)) });
  view.rerender(<SessionEndedNotice session={suspended.session} />);
  await act(async () => { expect(await suspended.restore()).toBe(false); });
  expect(screen.queryByText(i18n.t('auth.login.sessionEnded'))).not.toBeInTheDocument();
});
