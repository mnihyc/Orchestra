/** The actual environment Ports tab survives absent/late discovery and receives the native full projection. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import type * as EnvironmentApi from '@/api/environments';
import * as PortsApi from '@/api/ports';
import type { EnvironmentPorts } from '@/features/ports/presentation';
import { EnvironmentScreen, type EnvironmentTab } from '@/features/resources/environments/Environment';
import type { Environment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { applyQueryFrame } from '@/realtime/queryFeeds';
import type { FeedFrame } from '@/realtime/socket';

import { environment as nativePorts } from '../doubles/ports-wire';
import { portsProjection } from './ports-fixtures';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@/api/environments', async (load) => {
  const actual = await load<typeof EnvironmentApi>();
  return { ...actual, environmentQuery: (id: string) => ({ queryKey: actual.environmentKey(id), queryFn: () => Promise.resolve(environment), staleTime: Infinity }) };
});
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn(),
  Link: ({ children, className }: { children: ReactNode; className?: string }) => <a href="#environment" className={className}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/realtime/environmentFeed', () => ({ useEnvironmentFeed: () => undefined }));
vi.mock('@/features/resources/environments/acts', () => ({ useEnvironmentActs: () => ({
  page: () => ({ primary: null, edit: null, menu: [] }), keepAwake: () => null, dialogs: null,
}) }));
vi.mock('@/features/resources/environments/AgentReach', () => ({ AgentReach: () => null }));
vi.mock('@/features/ports/Request', () => ({ PortRequests: () => null }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title }: { title: ReactNode }) => <header><h1>{title}</h1></header> }));

const environment: Environment = { ...detailEnvironment, access: { ...detailEnvironment.access,
  permissions: ['environment.view', 'ports.view', 'ports.manage'] } };
const caches = new Set<QueryClient>();

function Scene() {
  const [tab, setTab] = useState<EnvironmentTab>('ports');
  return <EnvironmentScreen environmentId={environment.id} tab={tab} onTab={setTab} />;
}
function draw(theme: string) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  caches.add(cache);
  show(<QueryClientProvider client={cache}><div data-theme={theme}><Scene /></div></QueryClientProvider>);
  return cache;
}
function complete() {
  const projection = portsProjection('empty');
  projection.snapshot.environment_id = environment.id;
  const served = nativePorts(projection);
  return { ...served, targets: [], snapshot: { ...served.snapshot, listeners: [] } };
}
function receive(cache: QueryClient, data: Record<string, unknown>) {
  const frame: FeedFrame = { op: 'event', feed: `env:${environment.id}:ports`, id: 1, data };
  act(() => { applyQueryFrame(cache, PortsApi.portsKey(environment.id), frame); });
}

afterEach(async () => {
  cleanup();
  for (const cache of caches) { await cache.cancelQueries(); cache.clear(); }
  caches.clear();
  vi.restoreAllMocks();
  await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps missing discovery honest and the environment's other tabs usable (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const cache = draw(theme);
    await screen.findByTestId('ports-loading');
    const { snapshot, ...withoutSnapshot } = complete();
    const { discovery: _discovery, ...withoutDiscovery } = snapshot;
    for (const data of [{}, withoutSnapshot, { ...withoutSnapshot, snapshot: withoutDiscovery }]) receive(cache, data);
    expect(screen.getAllByText(i18n.t('ports.waitingDiscovery'), { exact: true })).toHaveLength(1);
    expect(screen.getByRole('button', { name: i18n.t('ports.refresh') })).toBeVisible();
    expect(screen.queryByText(i18n.t('ports.empty'), { exact: true })).not.toBeInTheDocument();
    expect(cache.getQueryData(PortsApi.portsKey(environment.id))).toBeUndefined();
    expect(screen.getByRole('heading', { level: 1, name: environment.name })).toBeVisible();
    const overview = screen.getByRole('tab', { name: i18n.t('environments.tabs.overview') });
    await userEvent.click(overview);
    expect(overview).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('heading', { level: 1, name: environment.name })).toBeVisible();
  });

  it(`renders a late native answer and retains it when a later observation is incomplete (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const cache = draw(theme);
    await screen.findByTestId('ports-loading');
    const served = complete();
    receive(cache, { ...served });
    await screen.findByTestId('ports-panel');
    expect(screen.queryByText(i18n.t('ports.waitingDiscovery'), { exact: true })).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('ports.empty'), { exact: true })).toBeVisible();
    const held = cache.getQueryData<EnvironmentPorts>(PortsApi.portsKey(environment.id));
    expect(held?.snapshot.discovery).toBe('native');
    receive(cache, { snapshot: served.snapshot });
    expect(cache.getQueryData(PortsApi.portsKey(environment.id))).toBe(held);
    expect(screen.getByTestId('ports-panel')).toBeVisible();
    expect(screen.getByRole('heading', { level: 1, name: environment.name })).toBeVisible();
  });

  it(`lets a waiting reader explicitly refresh without claiming that an answer has arrived (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const full = portsProjection('empty');
    full.snapshot.environment_id = environment.id;
    const refresh = vi.spyOn(PortsApi, 'refreshPorts').mockResolvedValue(full);
    draw(theme);
    await screen.findByTestId('ports-loading');
    expect(refresh).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('ports.refresh') }));
    await screen.findByTestId('ports-panel');
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(refresh).toHaveBeenCalledWith(environment.id);
    expect(screen.queryByText(i18n.t('ports.waitingDiscovery'), { exact: true })).not.toBeInTheDocument();
  });

  it(`keeps a refused initial discovery inside the tab (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const cache = draw(theme);
    await screen.findByTestId('ports-loading');
    receive(cache, { offline: true });
    const failure = await screen.findByRole('alert');
    expect(failure).not.toHaveTextContent(i18n.t('ui.error.partFailed'));
    expect(screen.queryByText(i18n.t('ports.empty'), { exact: true })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: environment.name })).toBeVisible();
    expect(screen.getByRole('tab', { name: i18n.t('environments.tabs.overview') })).toBeVisible();
  });
}
