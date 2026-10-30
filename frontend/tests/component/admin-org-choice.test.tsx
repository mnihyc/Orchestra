/** Organization choices use native q/cursor, safe current identity and the existing IDP save boundary. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Admin from '@/api/admin';
import type * as Client from '@/api/client';
import { ApiError, client } from '@/api/client';
import { IdentityProviders } from '@/features/admin/IdentityProviders';
import { OrganizationChoice } from '@/features/admin/OrganizationChoice';
import type { IdentityProvider, Org, OrgPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';
import { Toaster } from '@/ui/Toast';
import { id, me, org, userRef } from '../doubles/fixtures';

interface Options { params?: { path?: { id?: string }; query?: Record<string, unknown> }; signal?: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options?: Options) => Promise<unknown>>(),
  changed: vi.fn(), submit: vi.fn(), update: vi.fn<typeof Admin.updateIdentityProvider>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: async (path: string, options?: Options) => ({ data: await calls.read(path, options), response: new Response(null, { status: 200 }) }) } };
});
vi.mock('@/api/admin', async (load) => ({ ...await load<typeof Admin>(), updateIdentityProvider: calls.update }));

const owner = userRef(me(87, 'org-owner', 'Org Owner'));
const home = org(id(87, 99), 'parent', 'Parent org', owner);
const current = org(id(87, 1), 'current', 'Current department', owner, { parent_id: home.id, org_id: home.id, depth: 1, address: 'parent.current',
  path: [...home.path, { id: id(87, 1), kind: 'org', name: 'Current department', redacted: false }] });
const late = org(id(87, 2), 'research', 'Research department', owner, { parent_id: home.id, org_id: home.id, depth: 1, address: 'parent.research',
  path: [...home.path, { id: id(87, 2), kind: 'org', name: 'Research department', redacted: false }] });
const other = org(id(87, 3), 'beta', 'Beta org', owner);
const first = Array.from({ length: 13 }, (_, n) => org(id(88, n + 1), `org-${n + 1}`, `Organization ${n + 1}`, owner));
const page = (items: Org[], next_cursor: string | null = null): OrgPage => ({ items, next_cursor });
const failure = (code: 'internal' | 'authz.denied' = 'internal') => new ApiError({ type: `orch:error:${code}`, code,
  status: code === 'internal' ? 500 : 403, title: 'Private diagnostic', detail: 'Private diagnostic', request_id: 'org-choice' }, null);
const provider: IdentityProvider = { id: id(89, 1), key: 'review-sso', name: 'Review sign-in', issuer: 'https://login.example.test', client_id: 'orch',
  client_secret_set: true, scopes: ['openid', 'email'], claim_subject: 'sub', claim_email: 'email', claim_name: 'name', claim_username: 'preferred_username',
  allowed_email_domains: ['example.test'], auto_register: true, default_org_id: current.id, default_role: 'developer', enabled: true,
  callback_url: 'https://orch.example.test/api/v1/auth/oidc/review-sso/callback', revision: 7 };

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((accept) => { resolve = accept; });
  return { promise, resolve };
}
function Harness({ value = current.id, disabled = false }: { value?: string | null; disabled?: boolean }) {
  const [selected, setSelected] = useState(value);
  return <form onSubmit={(event) => { event.preventDefault(); calls.submit(selected); }}>
    <OrganizationChoice value={selected} disabled={disabled} onChange={(next) => { calls.changed(next); setSelected(next); }} />
    <select aria-label="Unrelated finite control"><option>One</option><option>Two</option></select>
    <button type="submit">Save fixture</button>
  </form>;
}
const clients: QueryClient[] = [];
function draw(ui: ReactNode) {
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } }); clients.push(queries);
  return { queries, ...render(<QueryClientProvider client={queries}><TooltipProvider><Toaster>{ui}</Toaster></TooltipProvider></QueryClientProvider>) };
}
const field = () => screen.getByRole('combobox', { name: i18n.t('admin.idp.defaultOrg') });
const list = () => within(screen.getByRole('listbox', { name: i18n.t('admin.idp.defaultOrg') }));
const reads = () => calls.read.mock.calls.filter(([path]) => path === '/admin/orgs');
const queries = () => reads().map(([, options]) => options?.params?.query);

beforeEach(async () => {
  await setLocale('en'); calls.read.mockReset(); calls.changed.mockReset(); calls.submit.mockReset(); calls.update.mockReset();
  client.session.setState({ readerVersion: 1 });
  calls.read.mockImplementation((path, options) => {
    if (path === '/admin/identity-providers') return Promise.resolve([provider]);
    if (path === '/nodes/{id}') return Promise.resolve(options?.params?.path?.id === late.id ? late : current);
    if (path === '/admin/orgs') return Promise.resolve(page(first, 'after-first'));
    return Promise.reject(new Error(`Unexpected read: ${path}`));
  });
  calls.update.mockResolvedValue({ ...provider, revision: 8 });
});
afterEach(() => { cleanup(); for (const query of clients) query.clear(); clients.length = 0; });

it('browses thirteen rows, retains current outside hits and follows an empty cursor page without duplicate reads', async () => {
  calls.read.mockImplementation((path, options) => {
    if (path === '/nodes/{id}') return Promise.resolve(current);
    const cursor = options?.params?.query?.cursor;
    return Promise.resolve(cursor === 'after-first' ? page([], 'after-empty') : cursor === 'after-empty' ? page([first[0]!, late]) : page(first, 'after-first'));
  });
  const user = userEvent.setup(); draw(<Harness />);
  await waitFor(() => expect(field()).toHaveValue(current.name)); expect(reads()).toHaveLength(0);
  await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(15));
  expect(list().getByRole('option', { name: i18n.t('admin.idp.noOrg') })).toHaveAccessibleDescription('');
  expect(list().getByRole('option', { name: current.name })).toHaveAttribute('aria-disabled', 'true');
  await user.tab(); expect(screen.getByRole('button', { name: i18n.t('resources.bulk.clear') })).toHaveFocus();
  await user.tab(); expect(screen.getByRole('button', { name: i18n.t('ui.pagination.more') })).toHaveFocus();
  await user.keyboard('{Enter}'); await waitFor(() => expect(reads()).toHaveLength(2));
  await waitFor(() => expect(field()).toHaveAttribute('aria-expanded', 'true'));
  expect(list().getAllByRole('option')).toHaveLength(15);
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(16));
  expect(screen.queryByRole('button', { name: i18n.t('ui.pagination.more') })).not.toBeInTheDocument();
  await waitFor(() => expect(field()).toHaveFocus()); expect(field()).toHaveAttribute('aria-expanded', 'true');
  const active = field().getAttribute('aria-activedescendant'); await user.keyboard('{ArrowDown}');
  expect(field().getAttribute('aria-activedescendant')).not.toBe(active); expect(calls.changed).not.toHaveBeenCalled();
  expect(queries()).toEqual([{ limit: 50 }, { limit: 50, cursor: 'after-first' }, { limit: 50, cursor: 'after-empty' }]);
  await user.click(list().getByRole('option', { name: late.name })); expect(field()).toHaveValue(late.name);
  expect(calls.changed).toHaveBeenCalledExactlyOnceWith(late.id); expect(calls.submit).not.toHaveBeenCalled();
  expect(calls.read.mock.calls.filter(([path]) => path === '/nodes/{id}')).toHaveLength(1);
});

it('uses native q, keeps the committed name on blur and retries one failed search from its reachable action', async () => {
  let failed = false;
  calls.read.mockImplementation((path, options) => {
    if (path === '/nodes/{id}') return Promise.resolve(current);
    if (options?.params?.query?.q === 'research') {
      if (!failed) { failed = true; return Promise.reject(failure()); }
      return Promise.resolve(page([late]));
    }
    return Promise.resolve(page(first));
  });
  const user = userEvent.setup(); draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await waitFor(() => expect(reads()).toHaveLength(1));
  await user.type(field(), 'research'); await screen.findByRole('alert');
  expect(screen.queryByText('Private diagnostic')).not.toBeInTheDocument(); expect(calls.changed).not.toHaveBeenCalled();
  const retry = screen.getByRole('button', { name: i18n.t('common.tryAgain') });
  await user.click(retry); await waitFor(() => expect(list().getByRole('option', { name: late.name })).toBeVisible());
  expect(queries()).toEqual([{ limit: 50 }, { q: 'research', limit: 50 }, { q: 'research', limit: 50 }]);
  await user.keyboard('{Escape}'); expect(field()).toHaveValue(current.name); expect(calls.submit).not.toHaveBeenCalled();
});

it('keeps clear reachable while an exact current read is pending and ignores its later name', async () => {
  const pending = deferred<Org>(); calls.read.mockImplementation(() => pending.promise);
  const user = userEvent.setup(); draw(<Harness />);
  expect(field()).toHaveValue(i18n.t('common.loading'));
  await user.click(screen.getByRole('button', { name: i18n.t('resources.bulk.clear') }));
  expect(field()).toHaveValue(i18n.t('admin.idp.noOrg')); expect(calls.changed).toHaveBeenCalledExactlyOnceWith(null);
  expect(field()).toHaveFocus(); expect(field()).toHaveAttribute('aria-expanded', 'false');
  await act(async () => { pending.resolve(current); await Promise.resolve(); });
  expect(field()).toHaveValue(i18n.t('admin.idp.noOrg')); expect(reads()).toHaveLength(0); expect(calls.submit).not.toHaveBeenCalled();
});

it('keeps clear reachable during search and isolates the response after the value is cleared', async () => {
  const pending = deferred<OrgPage>();
  calls.read.mockImplementation((path, options) => path === '/nodes/{id}' ? Promise.resolve(current)
    : options?.params?.query?.q === 'slow' ? pending.promise : Promise.resolve(page(first)));
  const user = userEvent.setup(); draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await user.type(field(), 'slow'); await waitFor(() => expect(reads()).toHaveLength(2));
  await user.click(screen.getByRole('button', { name: i18n.t('resources.bulk.clear') }));
  await act(async () => { pending.resolve(page([late])); await Promise.resolve(); });
  expect(field()).toHaveValue(i18n.t('admin.idp.noOrg')); expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  await waitFor(() => expect(field()).toHaveFocus()); expect(field()).toHaveAttribute('aria-expanded', 'false');
  expect(calls.changed).toHaveBeenCalledExactlyOnceWith(null); expect(calls.submit).not.toHaveBeenCalled();
});

it('drops the old current name on authority change and keeps the ID clearable after its exact read is refused', async () => {
  const user = userEvent.setup(); draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  calls.read.mockRejectedValue(failure('authz.denied'));
  await act(async () => { client.session.setState({ readerVersion: 2 }); await Promise.resolve(); });
  await waitFor(() => expect(field()).toHaveValue(i18n.t('settings.resourceChoice.unavailable')));
  expect(screen.queryByText(current.name)).not.toBeInTheDocument(); expect(screen.queryByText('Private diagnostic')).not.toBeInTheDocument();
  expect(calls.changed).not.toHaveBeenCalled(); expect(reads()).toHaveLength(0);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.bulk.clear') }));
  expect(calls.changed).toHaveBeenCalledExactlyOnceWith(null); expect(calls.submit).not.toHaveBeenCalled();
});

it('suppresses cached candidates after a refused next page and starts a fresh closed session on reader change', async () => {
  calls.read.mockImplementation((path, options) => path === '/nodes/{id}' ? Promise.resolve(current)
    : options?.params?.query?.cursor === 'after-first' ? Promise.reject(failure('authz.denied')) : Promise.resolve(page(first, 'after-first')));
  const user = userEvent.setup(); draw(<Harness value={null} />);
  await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(14));
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') })); await screen.findByRole('alert');
  await user.click(field()); expect(list().queryByRole('option', { name: first[0]!.name })).not.toBeInTheDocument();
  expect(screen.queryByText('Private diagnostic')).not.toBeInTheDocument(); expect(reads()).toHaveLength(2);
  await act(async () => { client.session.setState({ readerVersion: 2 }); await Promise.resolve(); });
  expect(field()).toHaveAttribute('aria-expanded', 'false'); expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  await user.click(field()); await waitFor(() => expect(reads()).toHaveLength(3));
  await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(14));
});

it('isolates obsolete query replies and IME Enter, then selects the current native result without submitting', async () => {
  const old = deferred<OrgPage>();
  calls.read.mockImplementation((_path, options) => options?.params?.query?.q === 'alpha' ? old.promise
    : Promise.resolve(page(options?.params?.query?.q === 'beta' ? [other] : first)));
  const user = userEvent.setup(); draw(<Harness value={null} />);
  await user.click(field()); await user.type(field(), 'alpha'); await waitFor(() => expect(reads()).toHaveLength(2));
  await user.clear(field()); await user.type(field(), 'beta'); await waitFor(() => expect(list().getByRole('option', { name: other.name })).toBeVisible());
  await act(async () => { old.resolve(page([late])); await Promise.resolve(); });
  expect(list().queryByRole('option', { name: late.name })).not.toBeInTheDocument();
  fireEvent.keyDown(field(), { key: 'Enter', keyCode: 229, isComposing: true });
  expect(calls.changed).not.toHaveBeenCalled(); expect(calls.submit).not.toHaveBeenCalled();
  await user.keyboard('{Enter}'); expect(calls.changed).toHaveBeenCalledExactlyOnceWith(other.id); expect(calls.submit).not.toHaveBeenCalled();
});

it('makes no closed candidate read or disabled field read and does not submit on closed Enter', async () => {
  const user = userEvent.setup(); const view = draw(<Harness value={null} disabled />);
  expect(field()).toBeDisabled(); expect(calls.read).not.toHaveBeenCalled(); view.unmount();
  draw(<Harness value={null} />); expect(reads()).toHaveLength(0);
  await user.click(field()); await waitFor(() => expect(reads()).toHaveLength(1)); await user.keyboard('{Escape}{Enter}');
  expect(calls.submit).not.toHaveBeenCalled(); expect(calls.changed).not.toHaveBeenCalled();
});

for (const clear of [false, true]) it(`keeps the real IDP role/secret draft and writes ${clear ? 'null organization and role' : 'the department ID'} only on explicit Save`, async () => {
  calls.read.mockImplementation((path, options) => Promise.resolve(path === '/admin/identity-providers' ? [provider] : path === '/nodes/{id}' ? current
    : page(options?.params?.query?.q === 'research' ? [late] : first)));
  const saved = deferred<IdentityProvider>(); calls.update.mockReturnValue(saved.promise);
  const user = userEvent.setup(); draw(<IdentityProviders />);
  await user.click(await screen.findByRole('button', { name: i18n.t('admin.idp.menuOf', { name: provider.name }) }));
  await user.click(screen.getByRole('menuitem', { name: i18n.t('admin.idp.edit') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('admin.idp.editTitle', { name: provider.name }) });
  await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.type(within(dialog).getByLabelText(i18n.t('admin.idp.secret')), 'replacement-secret-draft');
  if (clear) await user.click(screen.getByRole('button', { name: i18n.t('resources.bulk.clear') }));
  else {
    await user.click(field()); await user.type(field(), 'research');
    await waitFor(() => expect(list().getByRole('option', { name: late.name })).toBeVisible());
    await user.keyboard('{Enter}'); expect(field()).toHaveValue(late.name);
  }
  expect(calls.update).not.toHaveBeenCalled(); expect(within(dialog).getByLabelText(i18n.t('admin.idp.secret'))).toHaveValue('replacement-secret-draft');
  await user.click(within(dialog).getByRole('button', { name: i18n.t('admin.idp.save') }));
  await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(provider.id, {
    key: provider.key, name: provider.name, issuer: provider.issuer, client_id: provider.client_id, client_secret: 'replacement-secret-draft',
    scopes: provider.scopes, claim_subject: provider.claim_subject, claim_email: provider.claim_email, claim_name: provider.claim_name,
    claim_username: provider.claim_username, allowed_email_domains: provider.allowed_email_domains, auto_register: true,
    default_org_id: clear ? null : late.id, default_role: clear ? null : 'developer', enabled: true, expected_revision: provider.revision,
  }));
  expect(field()).toBeDisabled();
  await act(async () => { saved.resolve({ ...provider, revision: 8, default_org_id: clear ? null : late.id, default_role: clear ? null : 'developer' }); await Promise.resolve(); });
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
});
