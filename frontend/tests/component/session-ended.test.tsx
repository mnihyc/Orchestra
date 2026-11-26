/** A real refused restore distinguishes a previous device session from a first entrance. */
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
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

  it.each(['expired','revoked','account_unavailable'] as const)(`keeps a native %s result attached to its former session and clears its open facts on replacement in ${locale}`,async(cause)=>{
    useAppearance.setState({locale});await setLocale(locale);
    const at='2026-11-15T04:00:00Z';
    const fetch=vi.fn(()=>Promise.resolve(Response.json({type:'orch:error:auth.invalid_token',code:'auth.invalid_token',title:'Sign in',detail:'Sign in',status:401,request_id:'test',session_end:{cause,...(cause==='account_unavailable'?{}:{at_if_known:at})}},{status:401,headers:{'content-type':'application/problem+json'}})));
    const current=createApiClient({...options,fetch});current.adopt(grant());
    const view=show(<SessionEndedNotice session={current.session}/>);
    await act(async()=>{expect(await current.renew()).toBe(false);});
    const word=cause==='account_unavailable'?'accountUnavailable':cause;
    expect(screen.getByText(i18n.t(`auth.login.sessionEnd.${word}`))).toBeVisible();
    fireEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
    expect(screen.getByRole('dialog')).toBeVisible();
    expect(screen.getByText(i18n.t('auth.login.sessionEnd.reason'))).toBeVisible();
    if(cause==='account_unavailable')expect(within(screen.getByLabelText(i18n.t('factDisclosure.groups.recordedOutcome'))).getByText(i18n.t('factDisclosure.notReported'))).toBeVisible();
    else expect(view.baseElement.querySelector('time')?.dateTime).toBe(at);
    expect(fetch).toHaveBeenCalledTimes(1);
    act(()=>current.adopt({...grant(),user:me(2,'replacement','Replacement')}));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('button',{name:i18n.t('common.details')})).not.toBeInTheDocument();
  });

  it(`leaves missing session facts unreported instead of rebuilding them from a current expiry default in ${locale}`,async()=>{
    useAppearance.setState({locale});await setLocale(locale);
    const current=createApiClient(options);current.adopt(grant());
    show(<SessionEndedNotice session={current.session}/>);
    await act(async()=>{expect(await current.renew()).toBe(false);});
    expect(screen.getByText(i18n.t('auth.login.sessionEnded'))).toBeVisible();
    fireEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
    expect(within(screen.getByLabelText(i18n.t('factDisclosure.groups.recordedOutcome'))).getAllByText(i18n.t('factDisclosure.notReported'))).toHaveLength(2);
    expect(screen.queryByText(i18n.t('auth.login.sessionEnd.expired'))).not.toBeInTheDocument();
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
