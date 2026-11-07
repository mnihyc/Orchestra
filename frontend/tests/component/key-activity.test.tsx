/** The actual lazy activity view keeps current native privacy and explicit page ownership. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Client from '@/api/client';
import type * as Router from '@tanstack/react-router';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { KeyActivity } from '@/features/settings/KeyActivity';
import { Tokens } from '@/features/settings/personal/Tokens';
import type { ApiTokenActivity, ApiTokenActivityPage, ApiTokenInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const get = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: get } }; });
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a> }));
const row: ApiTokenActivity = { id: 'activity-1', at: '2026-10-01T12:00:00Z', operation: 'DocumentOpsCreate', selection: 'DocumentOpsCreate', method: 'POST', route: '/documents/{id}/ops', status: 403, error_code: 'authz.denied', replayed: true, duration_ms: 17, request_id: 'activity-request', ip: null,
  facets: [{ operation: 'DocumentOpsCreate', action: 'write', resource_kind: 'document', target: { kind: 'document', id: 'disclosed-id' }, target_name: null }] };
const page: ApiTokenActivityPage = { items: [row], next_cursor: null };
const token: ApiTokenInfo = { id: 'owned-key', name: 'Scoped work', rules: [], scopes: [], node: null, created_at: row.at, expires_at: null, last_used_at: null };
beforeEach(() => get.mockReset().mockResolvedValue({ data: page, response: new Response(null, { status: 200 }) }));
afterEach(async () => { cleanup(); await setLocale('en'); });

it('keeps personal activity cold through list and scope review, then reads exactly once on explicit selection', async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(['tokens'], [token]);
  show(<QueryClientProvider client={client}><Tokens /></QueryClientProvider>);
  const shown = await screen.findByRole('row', { name: /Scoped work/ });
  expect(get).not.toHaveBeenCalled();
  await userEvent.click(within(shown).getByRole('button', { name: i18n.t('operationKeys.review') }));
  expect(get).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('radio', { name: i18n.t('operationKeys.activity.title') }));
  expect(await screen.findByRole('table', { name: i18n.t('operationKeys.activity.title') })).toBeVisible();
  expect(get).toHaveBeenCalledOnce();
  expect(get.mock.calls[0]?.[0]).toBe('/me/tokens/{id}/activity');
  client.clear();
});

for (const locale of ['en', 'zh-CN'] as const) it(`shows native status/replay and keeps null names/IP undisclosed in ${locale}`, async () => {
  await setLocale(locale); show(<KeyActivity tokenId="owned-key" />);
  const table = await screen.findByRole('table', { name: i18n.t('operationKeys.activity.title') });
  expect(table).toHaveTextContent('403'); expect(table).toHaveTextContent('authz.denied'); expect(table).toHaveTextContent(i18n.t('operationKeys.activity.replayed'));
  await userEvent.click(within(table).getByText(i18n.t('operationKeys.activity.details')));
  expect(within(table).getByText(new RegExp(i18n.t('operationKeys.activity.undisclosedName')))).toBeVisible();
  expect(within(table).getByText(new RegExp(i18n.t('operationKeys.activity.undisclosedIp')), { selector: 'p' })).toBeVisible();
  expect(table).toHaveTextContent('activity-request'); expect(get).toHaveBeenCalledOnce();
});

it('requests only the org endpoint and passes the native next cursor unchanged', async () => {
  get.mockResolvedValueOnce({ data: { ...page, next_cursor: 'opaque|org|snapshot' }, response: new Response(null, { status: 200 }) })
    .mockResolvedValueOnce({ data: { items: [{ ...row, id: 'activity-2', operation: 'SecondOperation', selection: null }], next_cursor: null }, response: new Response(null, { status: 200 }) });
  show(<KeyActivity tokenId="owned-key" orgId="actual-org" />);
  await screen.findByRole('table');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  expect(await screen.findByText('SecondOperation')).toBeVisible();
  expect(screen.queryByText('DocumentOpsCreate')).not.toBeInTheDocument();
  expect(get).toHaveBeenCalledTimes(2);
  expect(get.mock.calls[1]).toEqual(['/orgs/{id}/tokens/{token_id}/activity', expect.objectContaining({ params: { path: { id: 'actual-org', token_id: 'owned-key' }, query: { limit: 12, cursor: 'opaque|org|snapshot' } } })]);
});

it('keeps a denied read as an error and allows only an explicit read retry', async () => {
  get.mockResolvedValueOnce({ error: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: 'Denied', status: 403, request_id: 'read-denied' }, response: new Response(null, { status: 403 }) });
  show(<KeyActivity tokenId="owned-key" />);
  await screen.findByRole('alert');
  expect(screen.queryByText(i18n.t('operationKeys.activity.none'))).not.toBeInTheDocument();
  expect(get).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('operationKeys.activity.refresh') }));
  await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  expect(await screen.findByRole('table')).toBeVisible();
});

it('cancels the previous key read and never publishes its late page in the next key view', async () => {
  let aborted = false;
  let finish: (value: { data: ApiTokenActivityPage; response: Response }) => void = () => { throw new Error('The initial request did not start'); };
  get.mockImplementationOnce((_path: unknown, init: unknown) => {
    if (typeof init !== 'object' || init === null || !('signal' in init) || !(init.signal instanceof AbortSignal)) throw new Error('The actual read must carry an AbortSignal');
    init.signal.addEventListener('abort', () => { aborted = true; });
    return new Promise<{ data: ApiTokenActivityPage; response: Response }>((resolve) => { finish = resolve; });
  }).mockResolvedValueOnce({ data: { ...page, items: [{ ...row, operation: 'NewKeyOperation', selection: null }] }, response: new Response(null, { status: 200 }) });
  const shown = show(<KeyActivity tokenId="old-key" />);
  await waitFor(() => expect(get).toHaveBeenCalledOnce());
  shown.rerender(<KeyActivity tokenId="new-key" />);
  expect(await screen.findByText('NewKeyOperation')).toBeVisible();
  expect(aborted).toBe(true);
  finish({ data: { ...page, items: [{ ...row, operation: 'OldKeyOperation', selection: null }] }, response: new Response(null, { status: 200 }) });
  await waitFor(() => expect(screen.getByText('NewKeyOperation')).toBeVisible());
  expect(screen.queryByText('OldKeyOperation')).not.toBeInTheDocument();
});
