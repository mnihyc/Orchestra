/** Activity reports its actual saved-session readers, independently of the model connection's readiness. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AdoptionApi from '@/api/adoption';
import { harnessSessionsQuery } from '@/api/adoption';
import type * as ResourcesApi from '@/api/resources';
import { providerKey } from '@/api/resources';
import { ApiError } from '@/api/client';
import { ProviderScreen } from '@/features/resources/providers/Provider';
import { ProviderLocation } from '@/features/resources/providers/ProviderList';
import { ProviderActivityStatus } from '@/features/resources/adopt/Sessions';
import { EnvironmentStatusText } from '@/features/resources/environments/EnvironmentList';
import { TemporaryStatus } from '@/features/resources/environments/Temporary';
import { environmentChangesVisible } from '@/features/resources/environments/words';
import type { Environment, HarnessState, NativeSession, Provider } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailAgent, detailEnvironment, detailHost, detailMachineRef, detailUnitProvider } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<Provider>>(), host: vi.fn(), sessions: vi.fn<(provider: string, agent: string) => Promise<NativeSession[]>>() }));
vi.mock('@/api/resources', async (load) => { const actual = await load<typeof ResourcesApi>(); return { ...actual,
  providerQuery: (id: string) => ({ queryKey: actual.providerKey(id), queryFn: calls.read, staleTime: Infinity }),
  hostQuery: (id: string) => ({ queryKey: actual.hostKey(id), queryFn: () => { calls.host(id); return Promise.resolve({ ...detailHost, id, name: provider.runtime?.host?.name ?? detailHost.name }); }, staleTime: Infinity }) }; });
vi.mock('@/api/adoption', async (load) => { const actual = await load<typeof AdoptionApi>(); return { ...actual,
  harnessSessionsQuery: (provider: string, agent: string) => ({ ...actual.harnessSessionsQuery(provider, agent), queryFn: () => calls.sessions(provider, agent) }) }; });
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn(),
  Link: ({ to, params, children, ...attributes }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: Record<string, string> }) => <a {...attributes} href={Object.entries(params ?? {}).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/providers/acts', () => ({ useProviderActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/providers/Threads', () => ({ ProviderThreads: () => null }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title, facts }: { title: string; facts: ReactNode[] }) => <header><h1>{title}</h1>{facts}</header> }));

const other = { ...detailAgent, id: 'second-profile', name: 'Second research profile' };
const provider: Provider = { ...detailUnitProvider, agent: detailAgent, harnesses: [detailAgent, other].map((agent): HarnessState => ({ agent, machine: detailMachineRef, status: 'online', status_detail: null, sessions_open: 0, harness_version: null, last_active_at: null })) };
const appearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), oldTheme = document.documentElement.dataset.theme;
beforeEach(() => {
  vi.clearAllMocks(); calls.read.mockResolvedValue(provider); calls.sessions.mockReset();
  calls.sessions.mockRejectedValue(new ApiError({ type: 'orch:error:harness.sessions_unavailable', code: 'harness.sessions_unavailable', title: 'Sessions unavailable', detail: 'The agent could not list its saved sessions.', status: 503, request_id: 'history-read' }, null));
});
afterEach(async () => { cleanup(); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', wasDark); if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme; await setLocale('en'); });
function draw(content: ReactNode) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return { ...show(<QueryClientProvider client={cache}>{content}</QueryClientProvider>), cache };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps Activity's header consistent with failures and the accepted recovery (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), view = draw(<ProviderScreen providerId={provider.id} tab="activity" onTab={vi.fn()} />);
    await screen.findByRole('heading', { name: provider.name, level: 1 });
    expect(await screen.findByText(i18n.t('adopt.historyUnavailable'), { exact: true })).toBeVisible();
    expect(screen.queryByText(i18n.t('providers.phrase.ready'), { exact: true })).not.toBeInTheDocument();
    expect(screen.getAllByRole('alert')).toHaveLength(2); expect(calls.sessions).toHaveBeenCalledTimes(2);
    expect(view.cache.getQueryData<Provider>(providerKey(provider.id))?.status).toBe('ready');
    const heading = screen.getByRole('heading', { name: detailAgent.name });
    const section = heading.closest('section'); if (section === null) throw new Error('Each session reader needs its own section');
    calls.sessions.mockResolvedValue([]); await user.click(within(section).getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(calls.sessions).toHaveBeenCalledTimes(3));
    expect(await screen.findByText(i18n.t('adopt.historyPartial'), { exact: true })).toBeVisible(); expect(screen.getAllByRole('alert')).toHaveLength(1);
    act(() => { view.cache.setQueryData(harnessSessionsQuery(provider.id, other.id).queryKey, []); });
    expect(await screen.findByText(i18n.t('adopt.historyAvailable'), { exact: true })).toBeVisible(); expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.sessions).toHaveBeenCalledTimes(3);
  });
  it(`shows the actual host rather than a unit's opaque connector name (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), view = draw(<ProviderLocation provider={provider} />), host = provider.runtime?.host;
    if (!host) throw new Error('The fixture needs a reported runtime host');
    const link = screen.getByRole('link', { name: host.name }); expect(link).toHaveAttribute('href', `/resources/hosts/${host.id}`);
    expect(view.container).not.toHaveTextContent('9fca895b060d'); expect(calls.host).not.toHaveBeenCalled();
    await user.hover(link);
    const card = await screen.findByRole('heading', { name: host.name });
    expect(within(card).getByRole('link', { name: host.name })).toHaveAttribute('href', `/resources/hosts/${host.id}`);
    await waitFor(() => expect(calls.host).toHaveBeenCalledExactlyOnceWith(host.id));
    expect(calls.read).not.toHaveBeenCalled();
  });
}

it('does not read a hidden session list merely to compute a header', async () => {
  draw(<ProviderActivityStatus harnesses={[{ providerId: provider.id, providerName: provider.name, agent: detailAgent }]} />);
  await act(async () => { await Promise.resolve(); }); expect(calls.sessions).not.toHaveBeenCalled();
  expect(screen.queryByText(i18n.t('adopt.historyAvailable'))).not.toBeInTheDocument();
});

it.each(['managed', 'connected'] as const)('names a declared attachment machine ahead of an old unit record on %s placement', (placement) => {
  const attached: Provider = { ...provider, placement, machine: { ...detailMachineRef, id: 'attachment', name: 'Current attachment machine' },
    runtime_support: { ...provider.runtime_support, lifecycle: 'attach', commands_after_start: null, after_up: false } };
  draw(<ProviderLocation provider={attached} />);
  expect(screen.getByRole('link', { name: 'Current attachment machine' })).toHaveAttribute('href', '/resources/machines/attachment');
  expect(screen.queryByRole('link', { name: provider.runtime?.host?.name })).not.toBeInTheDocument(); expect(calls.read).not.toHaveBeenCalled();
});

it('keeps the existing authority gate on Activity readers', async () => {
  calls.read.mockResolvedValue({ ...provider, access: { ...provider.access, permissions: ['provider.view'] } });
  draw(<ProviderScreen providerId={provider.id} tab="activity" onTab={vi.fn()} />); await screen.findByRole('heading', { name: provider.name, level: 1 });
  expect(calls.sessions).not.toHaveBeenCalled(); expect(screen.queryByText(i18n.t('adopt.historyAvailable'))).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`uses an accepted lifecycle job ahead of its unit's last physical report (${locale})`, async () => {
  await setLocale(locale);
  const queued: Environment = { ...detailEnvironment, status: 'running', job: { id: 'update-job', kind: 'environment_recreate', state: 'queued', phase: 'notify', percent: 0 } };
  const view = draw(<EnvironmentStatusText environment={queued} />);
  expect(screen.getByText(i18n.t('environments.status.recreating'), { exact: true })).toBeVisible();
  expect(screen.queryByText(i18n.t('environments.status.running'), { exact: true })).not.toBeInTheDocument(); expect(environmentChangesVisible(queued)).toBe(false);
  const completed: Environment = { ...queued, job: { id: 'update-job', kind: 'environment_recreate', state: 'succeeded', phase: 'done', percent: 100 } };
  view.rerender(<EnvironmentStatusText environment={completed} />);
  expect(screen.getByText(i18n.t('environments.status.running'), { exact: true })).toBeVisible(); expect(environmentChangesVisible(completed)).toBe(true);
});

for (const locale of ['en', 'zh-CN'] as const) it(`shows a queued removal before its retained-file metadata arrives (${locale})`, async () => {
  await setLocale(locale);
  const queued: Environment = { ...detailEnvironment, status: 'running', removal: null, temporary: { expires_at: null, idle_hours: 24, on_archive: false },
    job: { id: 'delete-job', kind: 'environment_remove', state: 'queued', phase: 'notify', percent: 0 } };
  draw(<><EnvironmentStatusText environment={queued} /><TemporaryStatus environment={queued} /></>);
  expect(screen.getByText(i18n.t('environments.lifecycle.scheduled'), { exact: true })).toBeVisible();
  expect(screen.queryByText(i18n.t('environments.status.running'), { exact: true })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('temporary.keep') })).not.toBeInTheDocument(); expect(environmentChangesVisible(queued)).toBe(false);
});
