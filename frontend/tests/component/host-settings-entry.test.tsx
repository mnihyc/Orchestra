/** Host Edit enters the canonical settings rows; Rename alone remains a guarded quick dialog. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type AnchorHTMLAttributes, type ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { hostKey, updateHost } from '@/api/resources';
import { everythingQuery } from '@/api/everything';
import { HostScreen, type HostTab } from '@/features/resources/hosts/Host';
import { useHostActs } from '@/features/resources/hosts/acts';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { Menu } from '@/ui/Menu';
import { detailHost } from './resource-detail-fixtures';
import { inventoryRow } from '../doubles/everything-row';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn(), navigate: vi.fn() }));
vi.mock('@/api/resources', async (load) => { const actual = await load<typeof ResourcesApi>(); return { ...actual, updateHost: vi.fn(),
  hostQuery: (id: string) => ({ queryKey: actual.hostKey(id), queryFn: calls.read, staleTime: Infinity }) }; });
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useNavigate: () => calls.navigate,
  useLocation: () => ({ pathname: '/resources/hosts/host', search: {}, searchStr: '' }),
  Link: ({ to, params, children, ...attributes }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: Record<string, string> }) => <a {...attributes} href={Object.entries(params ?? {}).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/resources/ComponentUpdate', async (load) => ({ ...await load<object>(), useComponentUpdateAct: () => () => null }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title, actions }: { title: string; actions: ReactNode }) => <header><h1>{title}</h1>{actions}</header> }));

const appearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), previousTheme = document.documentElement.dataset.theme;
beforeEach(() => { vi.clearAllMocks(); calls.read.mockResolvedValue(detailHost); vi.mocked(updateHost).mockReset(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', wasDark); if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme; await setLocale('en'); });
function Scene() { const [tab, setTab] = useState<HostTab>('overview'); return <HostScreen hostId={detailHost.id} tab={tab} onTab={setTab} />; }
function draw(content: ReactNode) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return { ...show(<QueryClientProvider client={cache}>{content}</QueryClientProvider>), cache };
}
function Row({ host = detailHost }: { host?: typeof detailHost }) {
  const acts = useHostActs(), name = i18n.t('resources.act.menuOf', { name: host.name });
  return <><Menu label={name} entries={acts.list.of(host).entries} trigger={<Button>{name}</Button>} />{acts.dialogs}</>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens one host settings surface and shows the accepted Save without follow-up reads (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), view = draw(<Scene />);
    await user.click(await screen.findByRole('button', { name: i18n.t('resources.act.editButton') }));
    expect(screen.getByRole('tab', { name: i18n.t('hosts.tabs.settings') })).toHaveAttribute('aria-selected', 'true');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('switch', { name: i18n.t('hosts.settings.takeTitle') })).toBeVisible();
    expect(screen.getByRole('combobox', { name: i18n.t('hosts.settings.fullTitle') })).toBeVisible();
    const labels = screen.getByRole('textbox', { name: i18n.t('hosts.settings.labels') });
    fireEvent.change(labels, { target: { value: 'gpu' } });
    const accepted = { ...detailHost, labels: ['gpu'], revision: 2 };
    vi.mocked(updateHost).mockResolvedValue(accepted);
    await user.click(screen.getByRole('button', { name: i18n.t('hosts.settings.save') }));
    expect(await screen.findByText(i18n.t('resourceDetails.layout.saved'), { exact: true })).toBeVisible();
    expect(updateHost).toHaveBeenCalledExactlyOnceWith(detailHost.id, { expected_revision: 1, labels: ['gpu'] });
    expect(view.cache.getQueryData(hostKey(detailHost.id))).toEqual(accepted); expect(calls.read).toHaveBeenCalledTimes(1);
    fireEvent.change(labels, { target: { value: 'next draft' } });
    expect(screen.queryByText(i18n.t('resourceDetails.layout.saved'), { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('settings.compact.undo') })).not.toBeInTheDocument();
  });
}

it('routes the list Edit to named settings and keeps Rename as a name-only dialog', async () => {
  const user = userEvent.setup(); draw(<Row />);
  const menu = screen.getByRole('button', { name: i18n.t('resources.act.menuOf', { name: detailHost.name }) });
  await user.click(menu); await user.click(screen.getByRole('menuitem', { name: i18n.t('resources.act.edit') }));
  expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/hosts/$id', params: { id: detailHost.id }, search: { tab: 'settings' } });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(menu); await user.click(screen.getByRole('menuitem', { name: i18n.t('resources.act.rename') }));
  expect(await screen.findByRole('dialog', { name: i18n.t('resources.edit.renameTitle', { name: detailHost.name }) })).toBeVisible();
  expect(screen.getAllByRole('textbox')).toHaveLength(1); expect(screen.queryByRole('switch')).not.toBeInTheDocument(); expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(updateHost).not.toHaveBeenCalled();
});

it('keeps the served manage gate on list editing', async () => {
  const user = userEvent.setup(); draw(<Row host={{ ...detailHost, access: { ...detailHost.access, permissions: ['host.view'] } }} />);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.menuOf', { name: detailHost.name }) }));
  expect(screen.queryByRole('menuitem', { name: i18n.t('resources.act.edit') })).not.toBeInTheDocument();
  expect(screen.queryByRole('menuitem', { name: i18n.t('resources.act.rename') })).not.toBeInTheDocument(); expect(calls.navigate).not.toHaveBeenCalled();
});

it('keeps a newer observed host when the older save answer arrives', async () => {
  const user = userEvent.setup(), view = draw(<HostScreen hostId={detailHost.id} tab="settings" onTab={vi.fn()} />);
  let accept: (host: typeof detailHost) => void = () => undefined;
  vi.mocked(updateHost).mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }));
  const taking = await screen.findByRole('switch', { name: i18n.t('hosts.settings.takeTitle') }); await user.click(taking);
  await waitFor(() => expect(updateHost).toHaveBeenCalledTimes(1));
  const latest = { ...detailHost, revision: 3, name: 'Newer host name' };
  act(() => { view.cache.setQueryData(hostKey(detailHost.id), latest); });
  await act(async () => { accept({ ...detailHost, revision: 2, schedulable: false }); await Promise.resolve(); });
  await waitFor(() => expect(taking).toBeEnabled()); expect(view.cache.getQueryData(hostKey(detailHost.id))).toEqual(latest);
  expect(screen.queryByRole('button', { name: i18n.t('settings.compact.undo') })).not.toBeInTheDocument(); expect(calls.read).toHaveBeenCalledTimes(1);
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`joins the admitted host measurements into both loaded list projections without another read (${locale}/${theme})`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const fresh = { ...detailHost, revision: 2, capacity: { ...detailHost.capacity, memory_mb: 39000 }, declared: { ...detailHost.declared, memory_mb: 2048 } };
  const before = { ...detailHost, capacity: { ...detailHost.capacity, memory_mb: 31500 }, declared: { ...detailHost.declared, memory_mb: 0 } };
  calls.read.mockResolvedValue(fresh);
  const view = draw(<HostScreen hostId={detailHost.id} tab="overview" onTab={vi.fn()} />);
  const inventoryKey = everythingQuery(detailHost.owner?.id ?? 'reader', 'resources', { scope: 'mine', kinds: ['host'], q: '', owner: null, createdFrom: '', createdTo: '', sort: 'name', direction: 'asc', resource: { details: true } }, 'hosts').queryKey;
  const inventory = { pages: [{ items: [inventoryRow({ id: before.id, kind: 'host', name: before.name, resource: { kind: 'host', value: before } })], next_cursor: null }], pageParams: [null] };
  view.cache.setQueryData(['hosts', null], [before]); view.cache.setQueryData(inventoryKey, inventory);
  const repair = vi.spyOn(view.cache, 'invalidateQueries'), refetch = vi.spyOn(view.cache, 'refetchQueries');
  expect(await screen.findByRole('heading', { level: 1, name: fresh.name })).toBeVisible();
  await waitFor(() => expect(view.cache.getQueryData(['hosts', null])).toEqual([fresh]));
  const joined = view.cache.getQueryData<typeof inventory>(inventoryKey)?.pages[0]?.items[0]?.resource;
  expect(joined).toEqual({ kind: 'host', value: fresh }); expect(view.cache.getQueryData(hostKey(fresh.id))).toEqual(fresh);
  expect(calls.read).toHaveBeenCalledTimes(1); expect(repair).not.toHaveBeenCalled(); expect(refetch).not.toHaveBeenCalled();
});
