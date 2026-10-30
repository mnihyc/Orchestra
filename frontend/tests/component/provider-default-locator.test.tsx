/** Usable provider search owns its native pages; the surrounding setting owns every write. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { applyRow } from '@/api/cache';
import type * as Client from '@/api/client';
import { ApiError, client } from '@/api/client';
import { ProviderDefaultChoice } from '@/features/settings/ProviderChoice';
import type { EverythingRow, EverythingRowPage, Provider } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Dialog } from '@/ui/Dialog';
import { TooltipProvider } from '@/ui/Tooltip';
import { inventoryRow } from '../doubles/everything-row';
import { id, me, userRef } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';

interface Options { params?: { path?: { id?: string }; query?: Record<string, unknown> }; signal?: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options?: Options) => Promise<unknown>>(),
  changed: vi.fn(), edited: vi.fn(), submit: vi.fn(), create: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: async (path: string, options?: Options) => ({ data: await calls.read(path, options), response: new Response() }) } };
});
vi.mock('@/features/navigation/placeLabels', () => ({ useKnownPlaces: () => new Map([['hidden home', [{ id: home.id, name: 'Private cached home', above: [] }]]]) }));
vi.mock('@/features/resources/ResourceCreation', () => ({ ResourceCreateButton: ({ onCreated }: { onCreated?: (id: string) => void }) => {
  const [open, setOpen] = useState(false);
  return <><button type="button" onClick={() => setOpen(true)}>{i18n.t('resourceCreation.providers')}</button>
    {open && <Dialog open title="Existing provider creation" onOpenChange={setOpen}>
      <button type="button" onClick={() => { calls.create(); setOpen(false); onCreated?.(late.id); }}>Finish native creation</button>
    </Dialog>}</>;
} }));

const owner = userRef(me(94, 'provider-owner', 'Provider Owner'));
const home = { id: id(94, 1), kind: 'org' as const, name: 'Provider home', redacted: false };
const current = planProvider({ id: id(95, 1), name: 'Stored provider', owner, home, org_id: home.id });
const late = planProvider({ id: id(95, 2), name: 'Remote usable provider', owner, home, org_id: home.id });
const first = Array.from({ length: 13 }, (_, n) => planProvider({ id: id(96, n + 1), name: `Usable provider ${n + 1}`, owner, home, org_id: home.id }));
const entry = (provider: Provider): EverythingRow => inventoryRow({ id: provider.id, name: provider.name, kind: 'provider', parent_id: 'resources',
  my_access: 'take_part', resource: { kind: 'provider', value: provider } });
const page = (providers: Provider[], next_cursor: string | null = null): EverythingRowPage => ({ items: providers.map(entry), next_cursor });
const failure = (code: 'internal' | 'authz.denied' = 'internal') => new ApiError({ type: `orch:error:${code}`, code,
  status: code === 'internal' ? 500 : 403, title: 'Private diagnostic', detail: 'Private diagnostic', request_id: 'provider-choice' }, null);
const request = { scope: 'mine', parent: 'resources', kinds: 'provider', resource_details: true, resource_view: 'usable', sort: 'name', direction: 'asc', limit: 50 };
const label = 'Default provider';
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((accept) => { resolve = accept; });
  return { promise, resolve };
}
function Harness({ value = current.id, disabled = false }: { value?: string | null; disabled?: boolean }) {
  const [selected, setSelected] = useState(value);
  return <form onSubmit={(event) => { event.preventDefault(); calls.submit(); }}>
    <ProviderDefaultChoice shape={{ type: 'text', maxLength: null, pattern: null }} label={label} value={selected} placeholder="Default: none" disabled={disabled}
      onEdited={calls.edited} onChange={(next) => { calls.changed(next); setSelected(typeof next === 'string' ? next : null); }} />
    <select aria-label="Unrelated finite control"><option>One</option><option>Two</option></select>
    <button type="submit">Save fixture</button>
  </form>;
}
const clients: QueryClient[] = [];
function draw(ui: ReactNode) {
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } }); clients.push(queries);
  return { queries, ...render(<QueryClientProvider client={queries}><TooltipProvider>{ui}</TooltipProvider></QueryClientProvider>) };
}
const field = () => screen.getByRole('combobox', { name: label });
const list = () => within(screen.getByRole('listbox', { name: label }));
const reads = () => calls.read.mock.calls.filter(([path]) => path === '/everything');
const names = () => calls.read.mock.calls.filter(([path]) => path === '/providers/{id}');
const args = () => reads().map(([, options]) => options?.params?.query);
const untouched = () => { expect(calls.changed).not.toHaveBeenCalled(); expect(calls.edited).not.toHaveBeenCalled(); expect(calls.submit).not.toHaveBeenCalled(); };
const session = client.session.getState();
beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks(); calls.read.mockReset();
  client.session.setState({ userId: owner.id, readerVersion: 1 });
  calls.read.mockImplementation((path) => Promise.resolve(path === '/providers/{id}' ? current : page(first, 'after-first')));
});
afterEach(() => { cleanup(); for (const query of clients) query.clear(); clients.length = 0; client.session.setState(session); });

it.each(['en', 'zh-CN'] as const)('shows a provider label for an inner recovery or embeddings field (%s)', async (locale) => {
  await setLocale(locale);
  const name = i18n.t('calls.provider');
  draw(<ProviderDefaultChoice shape={{ type: 'id' }} value="" label={name} showLabel onChange={calls.changed} />);
  expect(screen.getByText(name, { selector: 'label' })).toBeVisible();
  expect(screen.getByRole('combobox', { name })).toBeVisible();
  untouched();
});

it('browses thirteen usable rows, follows an empty native page, deduplicates and restores input focus after the final pager disappears', async () => {
  calls.read.mockImplementation((path, options) => {
    if (path === '/providers/{id}') return Promise.resolve(current);
    const cursor = options?.params?.query?.cursor;
    return Promise.resolve(cursor === 'after-first' ? page([], 'after-empty') : cursor === 'after-empty' ? page([first[0]!, late]) : page(first, 'after-first'));
  });
  const user = userEvent.setup(); draw(<Harness />);
  await waitFor(() => expect(field()).toHaveValue(current.name)); expect(reads()).toHaveLength(0);
  await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(14));
  expect(list().getByRole('option', { name: current.name })).toHaveAttribute('aria-disabled', 'true');
  await user.tab(); expect(screen.getByRole('button', { name: i18n.t('ui.pagination.more') })).toHaveFocus();
  await user.keyboard('{Enter}'); await waitFor(() => expect(reads()).toHaveLength(2));
  await waitFor(() => expect(field()).toHaveFocus()); expect(list().getAllByRole('option')).toHaveLength(14);
  await user.tab(); expect(screen.getByRole('button', { name: i18n.t('ui.pagination.more') })).toHaveFocus();
  await user.keyboard('{Enter}'); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(15));
  expect(screen.queryByRole('button', { name: i18n.t('ui.pagination.more') })).not.toBeInTheDocument();
  await waitFor(() => expect(field()).toHaveFocus()); expect(field()).toHaveAttribute('aria-expanded', 'true');
  const active = field().getAttribute('aria-activedescendant'); await user.keyboard('{ArrowDown}'); expect(field().getAttribute('aria-activedescendant')).not.toBe(active);
  expect(args()).toEqual([request, { ...request, cursor: 'after-first' }, { ...request, cursor: 'after-empty' }]); untouched();
  await user.click(list().getByRole('option', { name: late.name })); expect(field()).toHaveValue(late.name);
  expect(calls.changed).toHaveBeenCalledExactlyOnceWith(late.id); expect(calls.edited).toHaveBeenCalledTimes(1); expect(calls.submit).not.toHaveBeenCalled();
  expect(names()).toHaveLength(1);
});

it('uses native q and one explicit retry, then clears the query and restores the committed name without editing the setting', async () => {
  let failed = false;
  calls.read.mockImplementation((path, options) => {
    if (path === '/providers/{id}') return Promise.resolve(current);
    if (options?.params?.query?.q === 'remote') {
      if (!failed) { failed = true; return Promise.reject(failure()); }
      return Promise.resolve(page([late]));
    }
    return Promise.resolve(page(first));
  });
  const user = userEvent.setup(); draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await waitFor(() => expect(reads()).toHaveLength(1)); await user.type(field(), 'remote');
  await screen.findByRole('alert'); expect(screen.queryByText('Private diagnostic')).not.toBeInTheDocument(); untouched();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(list().getByRole('option', { name: late.name })).toBeVisible());
  expect(args()).toEqual([request, { ...request, q: 'remote' }, { ...request, q: 'remote' }]);
  await user.clear(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(14)); expect(reads()).toHaveLength(3);
  await user.keyboard('{Escape}{Enter}'); expect(field()).toHaveValue(current.name); expect(field()).toHaveAttribute('aria-expanded', 'false'); untouched();
});

it('can select another admitted provider while the independent current-name read is pending, without accepting its later name', async () => {
  const pending = deferred<Provider>(); calls.read.mockImplementation((path) => path === '/providers/{id}' ? pending.promise : Promise.resolve(page([late])));
  const user = userEvent.setup(); draw(<Harness />); expect(field()).toHaveValue(i18n.t('places.providerDefault.loading'));
  await user.click(field()); await user.click(await screen.findByRole('option', { name: late.name }));
  await act(async () => { pending.resolve(current); await Promise.resolve(); });
  expect(field()).toHaveValue(late.name); expect(calls.changed).toHaveBeenCalledExactlyOnceWith(late.id); expect(names()).toHaveLength(1); expect(calls.submit).not.toHaveBeenCalled();
});

it.each([403, 404])('keeps a private current provider Managed after %i even when a cached usable page contains it', async (status) => {
  calls.read.mockImplementation((path) => Promise.resolve(path === '/providers/{id}' ? current : page([current, late])));
  const user = userEvent.setup(), view = draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(2)); await user.keyboard('{Escape}');
  calls.read.mockImplementation((path) => path === '/providers/{id}' ? Promise.reject(new ApiError({type:'about:blank', title:'Private', status, code:status===403?'authz.denied':'not_found', detail:'Private',request_id:'private-provider'},null)) : Promise.resolve(page([current, late])));
  await act(async () => { await view.queries.refetchQueries({ queryKey: ['provider', current.id] }); });
  expect(view.queries.getQueryCache().find({ queryKey: ['provider', current.id, 'default-choice', `${owner.id}:1`] })?.state.status).toBe('error');
  await waitFor(() => expect(field()).toHaveValue(i18n.t('settings.compact.managed'))); expect(view.container).not.toHaveTextContent(current.id);
  await user.click(field()); expect(list().queryByRole('option', { name: current.name })).not.toBeInTheDocument();
  expect(list().getByRole('option', { name: i18n.t('settings.compact.managed') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', {name:i18n.t('common.tryAgain')})).not.toBeInTheDocument();
  expect(screen.queryByText('Private diagnostic')).not.toBeInTheDocument(); untouched();
});

it('recovers a closed current-name failure with exactly one read and keeps the candidate list closed on restored focus', async () => {
  calls.read.mockRejectedValueOnce(failure()).mockResolvedValue(current);
  const user = userEvent.setup(); draw(<Harness />); await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(field()).toHaveValue(current.name)); await waitFor(() => expect(field()).toHaveFocus());
  expect(field()).toHaveAttribute('aria-expanded', 'false'); expect(names()).toHaveLength(2); expect(reads()).toHaveLength(0); untouched();
});

it('ends the old reader session, drops its names and ignores its in-flight candidates', async () => {
  const pending = deferred<EverythingRowPage>();
  calls.read.mockImplementation((path) => path === '/providers/{id}' ? Promise.resolve(current) : pending.promise);
  const user = userEvent.setup(); draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await waitFor(() => expect(reads()).toHaveLength(1));
  calls.read.mockRejectedValue(failure('authz.denied'));
  await act(async () => { client.session.setState({ readerVersion: 2 }); await Promise.resolve(); });
  await waitFor(() => expect(field()).toHaveValue(i18n.t('settings.compact.managed')));
  await act(async () => { pending.resolve(page([late])); await Promise.resolve(); });
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument(); expect(screen.queryByText(current.name)).not.toBeInTheDocument();
  expect(screen.queryByText(late.name)).not.toBeInTheDocument(); expect(reads()).toHaveLength(1); expect(names()).toHaveLength(2); untouched();
});

it('hides cached rows on a permission failure from the next native page', async () => {
  calls.read.mockImplementation((_path, options) => options?.params?.query?.cursor === 'after-first' ? Promise.reject(failure('authz.denied')) : Promise.resolve(page(first, 'after-first')));
  const user = userEvent.setup(); draw(<Harness value={null} />);
  await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(13));
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') })); await screen.findByRole('alert');
  expect(list().queryAllByRole('option')).toHaveLength(0); expect(screen.queryByText(first[0]!.name)).not.toBeInTheDocument();
  expect(screen.queryByText('Private diagnostic')).not.toBeInTheDocument(); await waitFor(() => expect(field()).toHaveFocus());
  expect(reads()).toHaveLength(2); untouched();
});

it('isolates old queries and IME Enter, and retains raw provider identity on deliberate keyboard selection', async () => {
  const pending = deferred<EverythingRowPage>();
  calls.read.mockImplementation((_path, options) => options?.params?.query?.q === 'older' ? pending.promise : Promise.resolve(page(options?.params?.query?.q === 'remote' ? [late] : first)));
  const user = userEvent.setup(); draw(<Harness value={null} />);
  await user.click(field()); await user.type(field(), 'older'); await waitFor(() => expect(reads()).toHaveLength(2));
  await user.clear(field()); await user.type(field(), 'remote'); await waitFor(() => expect(list().getByRole('option', { name: late.name })).toBeVisible());
  await act(async () => { pending.resolve(page([current])); await Promise.resolve(); });
  expect(list().queryByRole('option', { name: current.name })).not.toBeInTheDocument();
  fireEvent.keyDown(field(), { key: 'Enter', keyCode: 229, isComposing: true }); untouched();
  await user.keyboard('{Enter}'); expect(calls.changed).toHaveBeenCalledExactlyOnceWith(late.id); expect(calls.submit).not.toHaveBeenCalled();
});

it('keeps unset/default and disabled states honest and does not rewrite an already selected eligible provider', async () => {
  const disabled = draw(<Harness disabled />); expect(field()).toBeDisabled(); expect(calls.read).not.toHaveBeenCalled(); disabled.unmount();
  const blank = draw(<Harness value={null} />); expect(field()).toHaveValue(''); expect(field()).toHaveAttribute('placeholder', 'Default: none'); expect(calls.read).not.toHaveBeenCalled(); blank.unmount();
  calls.read.mockImplementation((path) => Promise.resolve(path === '/providers/{id}' ? current : page([current])));
  const user = userEvent.setup(); draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await user.click(await screen.findByRole('option', { name: current.name }));
  expect(field()).toHaveValue(current.name); untouched();
});

it('does not invent choices or an empty-inventory claim from path-only rows, and still follows their native cursor', async () => {
  const hidden = inventoryRow({ id: current.id, name: null, kind: 'provider', my_access: 'path_only', resource: null });
  calls.read.mockImplementation((_path, options) => Promise.resolve(options?.params?.query?.cursor === 'continue' ? page([late]) : { items: [hidden], next_cursor: 'continue' }));
  const user = userEvent.setup(); draw(<Harness value={null} />); await user.click(field());
  await screen.findByRole('button', { name: i18n.t('ui.pagination.more') }); expect(list().queryAllByRole('option')).toHaveLength(0);
  expect(screen.queryByText(current.id)).not.toBeInTheDocument(); expect(screen.queryByText(i18n.t('places.providerDefault.empty'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  await waitFor(() => expect(list().getByRole('option', { name: late.name })).toBeVisible()); untouched();
});

for (const admitted of [false, true]) it(`keeps creation outside the popup and ${admitted ? 'offers' : 'withholds'} its result according to one subsequent native usable answer`, async () => {
  calls.read.mockImplementation(() => Promise.resolve(page(calls.create.mock.calls.length > 0 && admitted ? [late] : [])));
  const user = userEvent.setup(); draw(<Harness value={null} />); await user.click(field());
  await user.click(await screen.findByRole('button', { name: i18n.t('resourceCreation.providers') }));
  const dialog = await screen.findByRole('dialog', { name: 'Existing provider creation' }); expect(dialog).toBeVisible();
  await user.click(within(dialog).getByRole('button', { name: 'Finish native creation' }));
  await waitFor(() => expect(reads()).toHaveLength(2)); expect(dialog).not.toBeInTheDocument(); untouched();
  await user.click(field());
  if (admitted) {
    await user.click(await screen.findByRole('option', { name: late.name })); expect(calls.changed).toHaveBeenCalledExactlyOnceWith(late.id);
  } else { await screen.findByText(i18n.t('places.providerDefault.empty')); expect(list().queryAllByRole('option')).toHaveLength(0); untouched(); }
  expect(reads()).toHaveLength(2);
});

it('does not reopen or steal focus when a pending page completes after Escape', async () => {
  const pending = deferred<EverythingRowPage>();
  calls.read.mockImplementation((_path, options) => options?.params?.query?.cursor === 'after-first' ? pending.promise : Promise.resolve(page(first, 'after-first')));
  const user = userEvent.setup(); draw(<Harness value={null} />); await user.click(field());
  await user.click(await screen.findByRole('button', { name: i18n.t('ui.pagination.more') })); await waitFor(() => expect(reads()).toHaveLength(2));
  await user.keyboard('{Escape}'); expect(field()).toHaveAttribute('aria-expanded', 'false');
  await act(async () => { pending.resolve(page([late])); await Promise.resolve(); });
  expect(field()).toHaveAttribute('aria-expanded', 'false'); expect(field()).toHaveFocus(); expect(screen.queryByRole('listbox')).not.toBeInTheDocument(); untouched();
});

it('preserves an admitted current identity while a native disabled update removes its eligibility', async () => {
  calls.read.mockImplementation((path) => Promise.resolve(path === '/providers/{id}' ? current : page([current, late])));
  const user = userEvent.setup(), view = draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(2));
  await act(async () => { applyRow(view.queries, { kind: 'provider', id: current.id, value: { ...current, status: 'disabled' } }); await Promise.resolve(); });
  expect(view.queries.getQueryCache().find({ queryKey: ['provider', current.id, 'default-choice', `${owner.id}:1`] })?.state.data).toMatchObject({ status: 'disabled' });
  await waitFor(() => expect(list().getByRole('option', { name: current.name })).toHaveAttribute('aria-disabled', 'true'));
  await user.keyboard('{Enter}'); untouched(); await user.keyboard('{Escape}'); expect(field()).toHaveValue(current.name); expect(reads()).toHaveLength(1);
});

it('shows Managed after a native withdrawal and rereads it on authority repair', async () => {
  const view = draw(<Harness />); await waitFor(() => expect(field()).toHaveValue(current.name));
  await act(async () => { applyRow(view.queries, { kind: 'provider', id: current.id, value: null }); await Promise.resolve(); });
  expect(view.queries.getQueryCache().find({ queryKey: ['provider', current.id, 'default-choice', `${owner.id}:1`] })?.state.data).toBeNull();
  await waitFor(() => expect(field()).toHaveValue(i18n.t('settings.compact.managed')));
  expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
  await act(async () => { await view.queries.invalidateQueries({queryKey:['provider',current.id]}); });
  await waitFor(() => expect(field()).toHaveValue(current.name));
  expect(names()).toHaveLength(2); expect(reads()).toHaveLength(0); untouched();
});

it('never fills a redacted provider home from the cached place names used for disambiguation', async () => {
  const concealed = { ...late, name: current.name, home: { ...home, name: null, redacted: true } };
  calls.read.mockImplementation((path) => Promise.resolve(path === '/providers/{id}' ? current : page([current, concealed])));
  const user = userEvent.setup(); draw(<Harness />); await user.click(field()); await waitFor(() => expect(list().getAllByRole('option')).toHaveLength(2));
  expect(list().getByRole('option', { name: /Provider home/u })).toBeVisible();
  expect(screen.queryByText(/Private cached home/u)).not.toBeInTheDocument(); expect(screen.queryByText(home.id)).not.toBeInTheDocument(); untouched();
});
