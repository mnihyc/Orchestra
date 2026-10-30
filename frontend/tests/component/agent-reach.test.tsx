/** Agent reach distinguishes withheld facts from empty lists, per-thread arrangements and a lost read. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { applyChanges, findAct } from '@/api/acts';
import type * as EnvironmentsApi from '@/api/environments';
import { AgentReach } from '@/features/resources/environments/AgentReach';
import type { AgentReachOverview, BrowserProfile, Environment, NetworkState, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { show } from './support';

const { read } = vi.hoisted(() => ({ read: vi.fn<() => Promise<AgentReachOverview>>() }));
vi.mock('@/api/environments', async (original) => {
  const actual = await original<typeof EnvironmentsApi>();
  return { ...actual, environmentReachQuery: (id: string) => ({ ...actual.environmentReachQuery(id), queryFn: read }) };
});
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
afterEach(() => { vi.clearAllMocks(); });

const NETWORK: NetworkState = { mode: 'restricted', required_mode: 'restricted', network_mode: 'bridge', networks: [], proxy_url: null, allowed_hosts: ['packages.example'], sources: [], revision: 'r1', always: [{ authority: 'orch.example', port: 443, protocol: 'wss', purpose: 'gateway' }], always_revision: 'a1', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null };
const ENVIRONMENT: Pick<Environment, 'id' | 'name' | 'access' | 'job'> = { id: 'workspace', name: 'Workspace', job: null, access: { admin_only: [], participating: true, permissions: ['environment.view', 'environment.manage', 'files.view', 'browser.view'], administers: false, through: [{ kind: 'owner' }] } };
const HOME = { id: 'web', kind: 'project', name: 'Web', redacted: false } as const;

function reach(more: Partial<AgentReachOverview> = {}): AgentReachOverview {
  return { network: NETWORK, secrets: { names: ['BUILD_TOKEN_NAME'], source: HOME }, mounts: [{ mount: { id: 'data', name: 'Data', address: 'acme.data' }, path: '/mnt/data', mode: 'read_only' }], browser_profiles: [{ name: 'work-profile' }], credentials: [
    { thread: { id: 'separate', kind: 'thread', name: 'Separate build', redacted: false }, placement: 'separated', expires_at: null },
    { thread: { id: 'forwarder', kind: 'thread', name: 'Forwarded build', redacted: false }, placement: 'key_separated', expires_at: null },
    { thread: { id: 'copy', kind: 'thread', name: 'Copied credential', redacted: false }, placement: 'credential_shared', expires_at: '2026-10-15T09:00:00Z' },
    { thread: { id: 'login', kind: 'thread', name: 'Handed login', redacted: false }, placement: 'handed_over', expires_at: null },
    { thread: { id: 'together', kind: 'thread', name: 'Owner’s machine', redacted: false }, placement: 'together', expires_at: null },
    { thread: { id: 'unbound', kind: 'thread', name: 'Never started', redacted: false }, placement: 'unbound', expires_at: null },
  ], ...more };
}

function draw(environment = ENVIRONMENT, compact = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const root = createRootRoute({ component: () => <AgentReach environment={environment} compact={compact} /> });
  const routes = [
    createRoute({ getParentRoute: () => root, path: '/resources/environments/$id', component: () => null }),
    createRoute({ getParentRoute: () => root, path: '/resources/mounts/$id', component: () => null }),
    createRoute({ getParentRoute: () => root, path: '/project/$projectId', component: () => null }),
    createRoute({ getParentRoute: () => root, path: '/thread/$threadId', component: () => null }),
  ];
  const router = createRouter({ routeTree: root.addChildren(routes), history: createMemoryHistory({ initialEntries: ['/'] }) });
  show(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>);
  return client;
}

describe('what agents here can reach', () => {
  for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps compact reach rows and opens repeated explanations only once at ${locale}/${theme}`, async () => {
    const beforeTheme = document.documentElement.getAttribute('data-theme');
    try {
      await setLocale(locale); document.documentElement.dataset.theme = theme;
      const credentials: AgentReachOverview['credentials'] = [
        { thread: { id: 'first', kind: 'thread', name: 'First build', redacted: false }, placement: 'separated', expires_at: null },
        { thread: { id: 'second', kind: 'thread', name: 'Second build', redacted: false }, placement: 'separated', expires_at: null },
      ];
      read.mockResolvedValue(reach({ credentials, network: { ...NETWORK, mode: 'open', required_mode: 'open' } }));
      draw(ENVIRONMENT, true);
      expect(await screen.findByRole('link', { name: 'First build' })).toBeVisible();
      expect(screen.getByRole('link', { name: 'Second build' })).toBeVisible();
      const explanation = i18n.t('agentReach.placement.separated');
      expect(screen.queryByText(explanation)).not.toBeInTheDocument();
      expect(screen.getAllByText(i18n.t('agentReach.arrangements.separated.name'))).toHaveLength(2);
      expect(screen.queryByText(i18n.t('egress.outboundTunnel'))).not.toBeInTheDocument();
      const question = i18n.t('help.hint.label', { term: i18n.t('agentReach.credentials') });
      await userEvent.click(screen.getByRole('button', { name: question }));
      expect(screen.getAllByText(explanation)).toHaveLength(1);
      expect(read).toHaveBeenCalledTimes(1);
    } finally {
      if (beforeTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.dataset.theme = beforeTheme;
      await setLocale('en');
    }
  });
  it('shows the real network and names, and keeps a different key placement for every visible thread', async () => {
    read.mockResolvedValue(reach());
    draw();
    expect(await screen.findByText('BUILD_TOKEN_NAME')).toBeVisible();
    expect(screen.getByText('orch.example:443')).toBeVisible();
    expect(screen.getByText('packages.example')).toBeVisible();
    expect(screen.getByRole('link', { name: 'Web' })).toHaveAttribute('href', '/project/web');
    expect(screen.getByRole('link', { name: 'Data' })).toHaveAttribute('href', '/resources/mounts/data');
    expect(screen.getByText('/mnt/data')).toBeVisible();
    expect(screen.getByText('Read only')).toBeVisible();
    expect(screen.getByText('work-profile')).toBeVisible();
    expect(screen.getByText('The key stays with the provider; the environment holds none of it.')).toBeVisible();
    expect(screen.getByText(/works only at Orch’s forwarder/)).toBeVisible();
    expect(screen.getByText('The environment holds a copy of the model credential.')).toBeVisible();
    expect(screen.getByText(/whole sign-in is held/)).toBeVisible();
    expect(screen.getByText(/run on one machine/)).toBeVisible();
    expect(screen.getByText(/has not run yet/)).toBeVisible();
    expect(screen.getAllByText('Copy expires')).toHaveLength(1);
    expect(screen.getByRole('link', { name: 'Copied credential' })).toHaveAttribute('href', '/thread/copy');
  });

  it('words nulls as missing permissions and never invents an empty-secret or Only Orch promise for a cut view', async () => {
    read.mockResolvedValue(reach({ secrets: { names: null, source: null }, mounts: null, browser_profiles: null, credentials: [], network: { ...NETWORK, always: [], allowed_hosts: [] } }));
    draw({ ...ENVIRONMENT, access: { ...ENVIRONMENT.access, participating: false, permissions: [] } }, true);
    expect(await screen.findByText('No thread you can view is bound here.')).toBeVisible();
    expect(screen.getAllByText(/^You need .+ to see this\.$/)).toHaveLength(3);
    expect(screen.getByText('Network details require a permission on this environment.')).toBeVisible();
    expect(screen.queryByText('Only Orch')).not.toBeInTheDocument();
    expect(screen.queryByText('No secret names are supplied to commands.')).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Open settings' })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Open browsers' })).not.toBeInTheDocument();
    expect(screen.queryByText('BUILD_TOKEN_NAME')).not.toBeInTheDocument();
  });

  it('shows loading, drops old facts after access is lost, and retries into the new empty response', async () => {
    const pending: { resolve?: (value: AgentReachOverview) => void } = {};
    const response = new Promise<AgentReachOverview>((resolve) => { pending.resolve = resolve; });
    read.mockReturnValueOnce(response);
    draw();
    const block = await screen.findByRole('region', { name: 'What agents here can reach' });
    expect(block).toHaveAttribute('aria-busy', 'true');
    await act(async () => { pending.resolve?.(reach()); await response; });
    expect(await within(block).findByText('BUILD_TOKEN_NAME')).toBeVisible();
    read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', title: 'Denied', code: 'authz.denied', status: 403, detail: 'The environment is no longer readable.', request_id: 'lost' }, null));
    await userEvent.click(within(block).getByRole('button', { name: 'Refresh' }));
    await waitFor(() => expect(within(block).queryByText('BUILD_TOKEN_NAME')).not.toBeInTheDocument());
    expect(within(block).getByText('Last read')).toBeVisible();
    read.mockResolvedValue(reach({ secrets: { names: [], source: HOME }, mounts: [], browser_profiles: [], credentials: [] }));
    await userEvent.click(within(block).getByRole('button', { name: /Try again/ }));
    expect(await within(block).findByText('No secret names are supplied to commands.')).toBeVisible();
    expect(within(block).getByText('No mounts are attached.')).toBeVisible();
    expect(within(block).getByText('No browser profiles reported.')).toBeVisible();
  });

  it('keeps an unknown arrangement as Not reported', async () => {
    read.mockResolvedValue(reach({ credentials: [{ thread: { id: 'future', kind: 'thread', name: 'Future arrangement', redacted: false }, placement: 'a_new_arrangement', expires_at: null }] }));
    draw();
    expect(await screen.findByText('Not reported')).toBeVisible();
    expect(screen.queryByText('The key stays with the provider; the environment holds none of it.')).not.toBeInTheDocument();
  });

  it('updates an open reach block through the common registry after inherited settings and profile names change', async () => {
    read.mockResolvedValue(reach());
    const client = draw();
    expect(await screen.findByText('BUILD_TOKEN_NAME')).toBeVisible();
    read.mockResolvedValue(reach({ secrets: { names: ['DEPLOY_TOKEN_NAME'], source: HOME } }));
    const setting: ResolvedSetting = { key: 'env.secret_names', value: ['DEPLOY_TOKEN_NAME'], source: { scope: 'project', node: HOME }, own: { value: ['DEPLOY_TOKEN_NAME'], locked: false, revision: 2 }, parts: [], locked_by: null, writable: true };
    const changedSetting = findAct('PUT', '/nodes/web/settings/env.secret_names', { input: { value: setting.value }, answer: setting });
    if (changedSetting === null) throw new Error('The setting act is not registered.');
    await act(async () => { await applyChanges(changedSetting.changes, client); });
    expect(await screen.findByText('DEPLOY_TOKEN_NAME')).toBeVisible();
    expect(screen.queryByText('BUILD_TOKEN_NAME')).not.toBeInTheDocument();
    read.mockResolvedValue(reach({ secrets: { names: ['DEPLOY_TOKEN_NAME'], source: HOME }, browser_profiles: [{ name: 'new-profile-name' }] }));
    const profile: BrowserProfile = { name: 'new-profile-name', browser: null, size_bytes: null, used_at: null };
    const changedProfile = findAct('PATCH', '/environments/workspace/browser-profiles/work-profile', { input: { name: profile.name }, answer: profile });
    if (changedProfile === null) throw new Error('The browser profile act is not registered.');
    await act(async () => { await applyChanges(changedProfile.changes, client); });
    expect(await screen.findByText('new-profile-name')).toBeVisible();
    expect(screen.queryByText('work-profile')).not.toBeInTheDocument();
    expect(read).toHaveBeenCalledTimes(3);
  });
});
