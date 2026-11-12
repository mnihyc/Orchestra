/** Real shared scope/activity and confirmation components, with only the HTTP boundary doubled. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { applyChanges, findAct } from '@/api/acts';
import { AdminSessions } from '@/features/admin/Credentials';
import { AdminTokens } from '@/features/admin/Tokens';
import { i18n, setLocale } from '@/i18n';
import { reachActivity, reachPerson, reachService, reachSession, reachToken } from '../doubles/admin-reach';
import type { HttpAnswer, HttpTransport } from '../doubles/http-wire';
import { me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>(), remove: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.get, DELETE: calls.remove } }; });
const before = client.session.getState();
const caches: QueryClient[] = [];
const answer = (data: unknown): HttpAnswer => ({ data, response: new Response(null, { status: 200 }) });
const primary = reachToken();
const service = reachToken({ info: { ...primary.info, id: 'service-key', name: 'Service key' }, owner: reachService });
function cache() {
  const c = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  c.setQueryData(['me'], { ...me(1, 'ada', 'Ada'), is_system_admin: true });
  c.setQueryData(['admin-mode'], { as_admin: true, available: true }); caches.push(c); return c;
}
beforeEach(() => {
  calls.get.mockReset().mockImplementation((path) => Promise.resolve(answer({ items: path.endsWith('/activity') ? [reachActivity] : path.endsWith('/sessions') ? [reachSession()] : [primary, service], next_cursor: null })));
  calls.remove.mockReset().mockImplementation(async (path, request) => {
    const cache = caches.at(-1); if (cache === undefined) throw new Error('No credential view owns the accepted act');
    const actual = path.replace(/\{(\w+)\}/gu, (_whole: string, key: string) => {
      const value = request.params?.path?.[key]; if (value === undefined) throw new Error('Missing native path parameter');
      return encodeURIComponent(value);
    });
    const accepted = findAct('DELETE', actual); if (accepted === null) throw new Error('Missing native act registration');
    await applyChanges(accepted.changes, cache);
    return { response: new Response(null, { status: 204 }) };
  });
});
afterEach(async () => { cleanup(); caches.splice(0).forEach((c) => c.clear()); client.session.setState(before); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows human/service ownership and reads human activity only on selection (${locale})`, async () => {
    await setLocale(locale); show(<QueryClientProvider client={cache()}><AdminTokens /></QueryClientProvider>);
    const table = await screen.findByRole('table', { name: i18n.t('admin.sections.tokens.title') });
    expect(table).toHaveTextContent('Ada'); expect(table).toHaveTextContent('Build service');
    expect(table).toHaveTextContent(i18n.t('tokens.neverUsed'));
    expect(table).toHaveTextContent(i18n.t('tokens.neverEnds'));
    expect(calls.get).toHaveBeenCalledOnce();
    await userEvent.click(within(table).getByRole('button', { name: primary.info.name }));
    const review = screen.getByRole('dialog', { name: primary.info.name });
    expect(review).toHaveTextContent(i18n.t('operationKeys.denyAll'));
    expect(calls.get).toHaveBeenCalledOnce();
    await userEvent.click(within(review).getByRole('radio', { name: i18n.t('operationKeys.activity.title') }));
    expect(await within(review).findByText('DocumentGet')).toBeVisible();
    expect(calls.get.mock.calls[1]).toEqual(['/admin/users/{id}/tokens/{token_id}/activity', expect.objectContaining({ params: { path: { id: reachPerson.id, token_id: primary.info.id }, query: { limit: 12, cursor: null } } })]);
    await userEvent.click(within(review).getByLabelText(i18n.t('common.close')));
    await userEvent.click(within(table).getByRole('button', { name: service.info.name }));
    const serviceReview = screen.getByRole('dialog', { name: service.info.name });
    expect(within(serviceReview).queryByRole('radio', { name: i18n.t('operationKeys.activity.title') })).not.toBeInTheDocument();
  });

  it(`confirms one per-person token and preserves the refusal for retry (${locale})`, async () => {
    await setLocale(locale); calls.get.mockResolvedValue(answer({ items: [primary], next_cursor: null }));
    calls.remove.mockRejectedValueOnce(new Error('offline'));
    show(<QueryClientProvider client={cache()}><AdminTokens person={reachPerson.id} /></QueryClientProvider>);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('tokens.revoke') }));
    let confirm = screen.getByRole('alertdialog');
    await userEvent.click(within(confirm).getByRole('button', { name: i18n.t('common.cancel') }));
    expect(calls.remove).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('tokens.revoke') }));
    confirm = screen.getByRole('alertdialog');
    const action = within(confirm).getByRole('button', { name: i18n.t('tokens.revokeConfirm', { name: primary.info.name }) });
    await userEvent.click(action);
    expect(await within(confirm).findByRole('alert')).toBeVisible();
    expect(confirm).toBeVisible();
    expect(calls.remove).toHaveBeenCalledExactlyOnceWith('/admin/users/{id}/tokens/{token_id}', { params: { path: { id: reachPerson.id, token_id: primary.info.id } } });
    calls.get.mockResolvedValue(answer({ items: [], next_cursor: null }));
    await userEvent.click(action);
    await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
    expect(await screen.findByText(i18n.t('adminCredentials.noTokens'))).toBeVisible();
  });

  it(`all sessions states the current-browser effect and sends one native set act (${locale})`, async () => {
    await setLocale(locale); show(<QueryClientProvider client={cache()}><AdminSessions person={reachPerson.id} /></QueryClientProvider>);
    const table = await screen.findByRole('table', { name: i18n.t('adminCredentials.sessions') });
    expect(table).toHaveTextContent('Chrome (Linux)'); expect(table).toHaveTextContent(i18n.t('security.sessions.thisOne'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('adminCredentials.endAll') }));
    const confirm = screen.getByRole('alertdialog');
    expect(confirm).toHaveTextContent(i18n.t('adminCredentials.endAllBody'));
    calls.get.mockResolvedValue(answer({ items: [], next_cursor: null }));
    await userEvent.click(within(confirm).getByRole('button', { name: i18n.t('adminCredentials.endAll') }));
    await waitFor(() => expect(calls.remove).toHaveBeenCalledExactlyOnceWith('/admin/users/{id}/sessions', { params: { path: { id: reachPerson.id } } }));
    expect(await screen.findByText(i18n.t('adminCredentials.noSessions'))).toBeVisible();
  });
}

it('does not turn a refused inventory into an empty list or try an own-account substitute', async () => {
  calls.get.mockResolvedValueOnce({ error: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: 'Denied', status: 403, request_id: 'credential-read' }, response: new Response(null, { status: 403 }) });
  show(<QueryClientProvider client={cache()}><AdminTokens /></QueryClientProvider>);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByText(i18n.t('adminCredentials.noTokens'))).not.toBeInTheDocument();
  expect(calls.get).toHaveBeenCalledOnce();
  expect(calls.get.mock.calls[0]?.[0]).toBe('/admin/tokens');
});

it('unmounts a previous reader’s credentials and ignores its late answer', async () => {
  client.session.setState({ userId: 'first', readerVersion: 1 });
  let finish: (result: HttpAnswer) => void = () => { throw new Error('No first read'); };
  calls.get.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; })).mockResolvedValue(answer({ items: [service], next_cursor: null }));
  show(<QueryClientProvider client={cache()}><AdminTokens /></QueryClientProvider>);
  await waitFor(() => expect(calls.get).toHaveBeenCalledOnce());
  act(() => client.session.setState({ userId: 'second', readerVersion: 2 }));
  expect(await screen.findByText(service.info.name)).toBeVisible();
  await act(async () => { finish(answer({ items: [primary], next_cursor: null })); await Promise.resolve(); });
  expect(screen.queryByText(primary.info.name)).not.toBeInTheDocument();
});
