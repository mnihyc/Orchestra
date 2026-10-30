/** Seven real detail compositions keep visible navigation, compact facts and unloaded secondary panels. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useEffect, useState, type ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { hostKey, machineKey, updateHost } from '@/api/resources';
import type * as EnvironmentsApi from '@/api/environments';
import { environmentKey } from '@/api/environments';
import type * as ToolsApi from '@/api/resource-tools';
import { mcpKey } from '@/api/resource-tools';
import { AgentScreen, type AgentTab } from '@/features/resources/agents/Profile';
import { EnvironmentScreen, type EnvironmentTab } from '@/features/resources/environments/Environment';
import { HostScreen, type HostTab } from '@/features/resources/hosts/Host';
import { Machine, type MachineTab } from '@/features/resources/machines/Machine';
import { TemplateScreen, type TemplateTab } from '@/features/resources/templates/Template';
import { McpScreen } from '@/features/resources/mcp/Preset';
import { MountScreen } from '@/features/resources/mounts/Mount';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type { Environment, EnvironmentSandboxReadiness } from '@/generated/api-types';
import { detailAgent, detailEnvironment, detailHost, detailMachine, detailMcp, detailMount, detailSchema, detailTemplate } from './resource-detail-fixtures';
import { show } from './support';

type Kind = 'agent' | 'machine' | 'environment' | 'host' | 'template' | 'mcp' | 'mount';
const kinds: readonly Kind[] = ['agent', 'machine', 'environment', 'host', 'template', 'mcp', 'mount'];
const creationKinds = { agent: 'agents', machine: 'machines', environment: 'environments', host: 'hosts', template: 'templates', mcp: 'mcp', mount: 'mounts' } as const;
const records = { agent: detailAgent, machine: detailMachine, environment: detailEnvironment, host: detailHost, template: detailTemplate, mcp: detailMcp, mount: detailMount };
const calls = vi.hoisted(() => ({ read: vi.fn(), hidden: vi.fn(), uses: vi.fn<() => Promise<{ scopes: []; environments: []; hidden: number }>>() }));
function read<K extends keyof typeof records>(kind: K) { calls.read(kind); return Promise.resolve(records[kind]); }
vi.mock('@/api/resources', async (load) => {
  const actual = await load<typeof ResourcesApi>();
  return { ...actual,
    updateHost: vi.fn(),
    agentQuery: (id: string) => ({ queryKey: actual.agentKey(id), queryFn: () => read('agent'), staleTime: Infinity }),
    machineQuery: (id: string) => ({ queryKey: actual.machineKey(id), queryFn: () => read('machine'), staleTime: Infinity }),
    hostQuery: (id: string) => ({ queryKey: actual.hostKey(id), queryFn: () => read('host'), staleTime: Infinity }),
    templateQuery: (id: string) => ({ queryKey: actual.templateKey(id), queryFn: () => read('template'), staleTime: Infinity }),
    harnessKindsQuery: { queryKey: ['detail-harness-kinds'], queryFn: () => Promise.resolve([]), staleTime: Infinity },
    driversQuery: { queryKey: ['detail-drivers'], queryFn: () => Promise.resolve([{ name: 'sample', title: 'Example driver', descriptor: { spec_schemas: { environment: detailSchema } } }]), staleTime: Infinity },
    checkTemplate: vi.fn(() => Promise.resolve({ valid: true, errors: [] })),
  };
});
vi.mock('@/api/environments', async (load) => {
  const actual = await load<typeof EnvironmentsApi>();
  return { ...actual, environmentQuery: (id: string) => ({ queryKey: actual.environmentKey(id), queryFn: () => read('environment'), staleTime: Infinity }) };
});
vi.mock('@/api/resource-tools', async (load) => {
  const actual = await load<typeof ToolsApi>();
  return { ...actual,
    mcpQuery: (id: string) => ({ queryKey: actual.mcpKey(id), queryFn: () => read('mcp'), staleTime: Infinity }),
    mountQuery: (id: string) => ({ queryKey: actual.mountKey(id), queryFn: () => read('mount'), staleTime: Infinity }),
    mountUsesQuery: (id: string) => ({ queryKey: ['detail-mount-uses', id], queryFn: calls.uses, staleTime: Infinity }),
  };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useNavigate: () => vi.fn(), useLocation: () => ({ pathname: '/resources/example', searchStr: '' }),
  Link: ({ children, to, params, className }: { children: ReactNode; to: string; params?: Record<string, string>; className?: string }) => <a className={className} href={Object.entries(params ?? {}).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/settings/sourceAddress', () => ({ useSourceOrgAddress: () => 'workshop' }));
vi.mock('@/realtime/environmentFeed', () => ({ useEnvironmentFeed: () => undefined }));
vi.mock('@/features/resources/agents/acts', () => ({ useAgentActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/machines/acts', () => ({ useMachineActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/hosts/acts', () => ({ useHostActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/templates/acts', () => ({ useTemplateActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/environments/acts', () => ({ useEnvironmentActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), keepAwake: () => null, dialogs: null }) }));
// Native editors and their writes have their own proofs. These readers mark when a secondary panel actually mounts.
function Secondary({ name }: { name: string }) { useEffect(() => { calls.hidden(name); }, [name]); return <p>{name}</p>; }
vi.mock('@/features/resources/machines/Policy', () => ({ MachinePolicyTab: () => <Secondary name="policy-reader" />, MachineLimits: () => <Secondary name="limits-reader" /> }));
vi.mock('@/features/resources/machines/Programs', () => ({ MachineProgramsTab: () => <Secondary name="program-reader" /> }));
vi.mock('@/features/resources/machines/Installation', async (load) => ({ ...await load<object>(), Installation: () => <Secondary name="installation-reader" /> }));
vi.mock('@/features/resources/environments/McpServers', () => ({ McpServers: () => <Secondary name="mcp-reader" /> }));
vi.mock('@/features/resources/environments/Doors', () => ({ EnvironmentDoor: () => <Secondary name="door-reader" /> }));
vi.mock('@/features/resources/environments/AgentReach', () => ({ AgentReach: () => <Secondary name="reach-reader" /> }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title, lead, facts, actions }: { title: string; lead?: ReactNode; facts?: ReactNode[]; actions?: ReactNode }) => <header><h1>{title}</h1>{lead}<div>{facts}</div>{actions}</header> }));

function Scene({ kind }: { kind: Kind }) {
  const [agent, setAgent] = useState<AgentTab>('overview'), [machine, setMachine] = useState<MachineTab>('overview');
  const [environment, setEnvironment] = useState<EnvironmentTab>('overview'), [host, setHost] = useState<HostTab>('overview'), [template, setTemplate] = useState<TemplateTab>('overview');
  switch (kind) {
    case 'agent': return <AgentScreen agentId={detailAgent.id} tab={agent} onTab={setAgent} />;
    case 'machine': return <Machine machineId={detailMachine.id} tab={machine} onTab={setMachine} />;
    case 'environment': return <EnvironmentScreen environmentId={detailEnvironment.id} tab={environment} onTab={setEnvironment} />;
    case 'host': return <HostScreen hostId={detailHost.id} tab={host} onTab={setHost} />;
    case 'template': return <TemplateScreen templateId={detailTemplate.id} tab={template} onTab={setTemplate} />;
    case 'mcp': return <McpScreen id={detailMcp.id} />;
    case 'mount': return <MountScreen id={detailMount.id} />;
  }
}
function draw(content: ReactNode, host?: typeof detailHost) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  if (host !== undefined) client.setQueryData(hostKey(host.id), host);
  return { ...show(<QueryClientProvider client={client}>{content}</QueryClientProvider>), client };
}
const previousAppearance = useAppearance.getState(), previousTheme = document.documentElement.dataset.theme, wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); calls.uses.mockResolvedValue({ scopes: [], environments: [], hidden: 2 }); });
afterEach(async () => { cleanup(); useAppearance.setState(previousAppearance); document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it.each(kinds)(`keeps %s to six visible tabs, one compact fact list and no diagnostic dump (${locale}/${theme})`, async (kind) => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const view = draw(<Scene kind={kind} />);
    expect(await screen.findByRole('heading', { level: 1, name: records[kind].name })).toBeVisible();
    const header = screen.getByRole('heading', { level: 1, name: records[kind].name }).closest('header');
    if (header === null) throw new Error('The resource needs its own page header');
    expect(within(header).queryByRole('button', { name: i18n.t(`resourceCreation.${creationKinds[kind]}`) })).not.toBeInTheDocument();
    const navigation = screen.getByRole('tablist');
    const tabs = within(navigation).getAllByRole('tab'); expect(tabs.length).toBeLessThanOrEqual(6); expect(tabs.length).toBeGreaterThanOrEqual(3);
    for (const tab of tabs) expect(tab).toBeVisible();
    expect(screen.queryByRole('button', { name: /^More$/i })).not.toBeInTheDocument();
    expect(view.container.querySelectorAll('[data-resource-facts]')).toHaveLength(1);
    expect(screen.queryByText('diagnostic-canary')).not.toBeInTheDocument();
    expect(calls.read).toHaveBeenCalledExactlyOnceWith(kind); expect(calls.hidden).not.toHaveBeenCalled(); expect(calls.uses).not.toHaveBeenCalled();
  });
}

it('opens mount Uses only when chosen and keeps the server visibility cut', async () => {
  const user = userEvent.setup(); draw(<Scene kind="mount" />); await screen.findByRole('heading', { name: detailMount.name });
  expect(calls.uses).not.toHaveBeenCalled();
  await user.click(screen.getByRole('tab', { name: i18n.t('resourceDetails.mount.uses') }));
  expect(await screen.findByText(i18n.t('resourceDetails.mount.hidden', { count: 2 }))).toBeVisible(); expect(calls.uses).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('tab', { name: i18n.t('resourceDetails.overview') }));
  await user.click(screen.getByRole('tab', { name: i18n.t('resourceDetails.mount.uses') })); expect(calls.uses).toHaveBeenCalledTimes(1);
});

it.each(['en', 'zh-CN'] as const)('keeps an MCP shell script out of ordinary facts until Details is opened (%s)', async (locale) => {
  await setLocale(locale); const user = userEvent.setup(), view = draw(<McpScreen id={detailMcp.id} />);
  await screen.findByRole('heading', { name: detailMcp.name });
  const script = 'printf "D443_SCRIPT"\nprintf "ready"\nexit 0';
  act(() => { view.client.setQueryData(mcpKey(detailMcp.id), { ...detailMcp, revision: 2, spec: { transport: 'stdio', command: ['sh', '-c', script] } }); });
  expect(await screen.findByText('sh', { exact: true })).toBeVisible(); expect(view.container).not.toHaveTextContent('D443_SCRIPT');
  await user.click(screen.getByText(i18n.t('common.details'), { exact: true }));
  expect(await screen.findByText(/D443_SCRIPT/)).toBeVisible();
});

it('keeps an environment terminal address inside Tools and does not mount its MCP reader', async () => {
  draw(<EnvironmentScreen environmentId={detailEnvironment.id} tab="terminal" onTab={vi.fn()} />);
  await screen.findByRole('heading', { name: detailEnvironment.name });
  expect(screen.getByRole('tab', { name: i18n.t('resourceDetails.layout.tools') })).toHaveAttribute('aria-selected', 'true');
  expect(await screen.findByText('door-reader')).toBeVisible(); expect(calls.hidden).toHaveBeenCalledExactlyOnceWith('door-reader');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it.each(['provisioning', 'recreating', 'moving'] as const)(`does not describe an active %s operation as another pending update (${locale}/${theme})`, async (status) => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const view = draw(<EnvironmentScreen environmentId={detailEnvironment.id} tab="overview" onTab={vi.fn()} />);
    await screen.findByRole('heading', { name: detailEnvironment.name, level: 1 });
    const updates: Environment['updates'] = [{ what: 'settings', name: 'Runtime definition', from: 'a'.repeat(64), to: 'b'.repeat(64) }];
    act(() => { view.client.setQueryData<Environment>(environmentKey(detailEnvironment.id), { ...detailEnvironment, status, updates, update_required: true }); });
    expect(await screen.findByText(i18n.t(`environments.status.${status}`), { exact: true })).toBeVisible();
    expect(screen.queryByText(i18n.t('environments.overview.behind'), { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('environments.list.updateRequired'), { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('resourceDetails.definitionChanged'), { exact: true })).not.toBeInTheDocument();
    act(() => { view.client.setQueryData<Environment>(environmentKey(detailEnvironment.id), { ...detailEnvironment, updates }); });
    expect(await screen.findByText(i18n.t('environments.overview.behind'), { exact: true })).toBeVisible();
    expect(screen.getByText(i18n.t('resourceDetails.definitionChanged'), { exact: true })).toBeVisible(); expect(calls.read).toHaveBeenCalledExactlyOnceWith('environment');
  });
  it(`shows accepted deletion once instead of the last Running report or the old temporary deadline (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const view = draw(<EnvironmentScreen environmentId={detailEnvironment.id} tab="overview" onTab={vi.fn()} />);
    await screen.findByRole('heading', { name: detailEnvironment.name, level: 1 });
    const removing: Environment = { ...detailEnvironment, status: 'running', temporary: { expires_at: null, idle_hours: 24, on_archive: false },
      removal: { phase: 'removing', deleted_at: null, deleted_by: detailEnvironment.owner, keep_days: 0, purge_at: null, purged_at: null, bytes: null, size_at: null, reason: null, revision: 1, can_inspect: true, can_restore: false, can_purge: false, job: null } };
    act(() => { view.client.setQueryData(environmentKey(detailEnvironment.id), removing); });
    expect(await screen.findByText(i18n.t('environments.lifecycle.removing'), { exact: true })).toBeVisible();
    expect(screen.getAllByText(i18n.t('environments.lifecycle.removing'), { exact: true })).toHaveLength(1);
    expect(screen.queryByText(i18n.t('environments.status.running'), { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('temporary.keep') })).not.toBeInTheDocument(); expect(screen.queryByRole('button', { name: i18n.t('temporary.export') })).not.toBeInTheDocument();
    expect(view.container.querySelector('[data-temporary-environment]')).toBeNull();
  });
}

it('keeps a host policy address inside Settings without reading runtimes or storage', async () => {
  draw(<HostScreen hostId={detailHost.id} tab="policy" onTab={vi.fn()} />); await screen.findByRole('heading', { name: detailHost.name });
  expect(screen.getByRole('tab', { name: i18n.t('hosts.tabs.settings') })).toHaveAttribute('aria-selected', 'true');
  expect(await screen.findByText('policy-reader')).toBeVisible(); expect(calls.hidden).toHaveBeenCalledExactlyOnceWith('policy-reader');
});

it('retains native machine program addresses in Registered and names their destination', async () => {
  draw(<Machine machineId={detailMachine.id} tab="programs" onTab={vi.fn()} />); await screen.findByRole('heading', { name: detailMachine.name });
  expect(screen.getByRole('tab', { name: i18n.t('machines.tabs.registered') })).toHaveAttribute('aria-selected', 'true');
  const link = screen.getByRole('link', { name: i18n.t('machinePrograms.title') }); expect(link).toHaveAttribute('href', '/resources/machines/machine?tab=programs');
  expect(await screen.findByText('program-reader')).toBeVisible(); expect(calls.hidden).toHaveBeenCalledExactlyOnceWith('program-reader');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps a machine access status once across tabs and remembers its dismissal (${locale}/${theme})`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const user = userEvent.setup(), view = draw(<Scene kind="machine" />);
  await screen.findByRole('heading', { name: detailMachine.name });
  const readiness: EnvironmentSandboxReadiness = { environment: detailEnvironment, machine: detailMachine, policy_manage: true,
    sandbox: { ready: true, requested: 'open', widest: 'open', source: 'machine_policy', machine_limit: 'open', template_limit: null, executor: 'orch-exec', enforceable: ['open'], agent_modes: [], reason: null } };
  act(() => { view.client.setQueryData(machineKey(detailMachine.id), { ...detailMachine, environment_sandboxes: [readiness, readiness] }); });
  const list = await screen.findByRole('list', { name: i18n.t('sandboxReadiness.summary') }); expect(within(list).getAllByRole('listitem')).toHaveLength(1);
  await user.click(screen.getByRole('tab', { name: i18n.t('machines.tabs.policy') }));
  expect(screen.getAllByRole('list', { name: i18n.t('sandboxReadiness.summary') })).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: i18n.t('sandboxReadiness.dismiss', { name: detailEnvironment.name }) }));
  await user.click(screen.getByRole('tab', { name: i18n.t('machines.tabs.overview') }));
  expect(screen.queryByRole('list', { name: i18n.t('sandboxReadiness.summary') })).not.toBeInTheDocument();
  act(() => { view.client.setQueryData(machineKey(detailMachine.id), { ...detailMachine, environment_sandboxes: [{ ...readiness, sandbox: { ...readiness.sandbox, ready: false, reason: 'Tool access changed' } }] }); });
  expect(await screen.findByText(i18n.t('sandboxReadiness.modeBlocked', { mode: i18n.t('sandboxReadiness.modes.open.name') }))).toBeVisible();
  expect(screen.queryByText('Tool access changed')).not.toBeInTheDocument();
});

it('shows readable parameter defaults with the native Defaults destination and bounded rows', async () => {
  draw(<AgentScreen agentId={detailAgent.id} tab="parameters" onTab={vi.fn()} />); await screen.findByRole('heading', { name: detailAgent.name });
  const first = screen.getByText('Worker option 1').closest<HTMLElement>('[data-setting-row]'); if (first === null) throw new Error('the first parameter has no setting row');
  expect(within(first).getByText('An adapter-declared option.')).toBeVisible();
  expect(within(first).getByRole('link', { name: i18n.t('resourceDetails.layout.openDefaults') })).toHaveAttribute('href', expect.stringContaining('setting=worker_field_0'));
  expect(screen.queryByText('worker_field_0', { exact: true })).not.toBeInTheDocument(); expect(screen.queryByText('Worker option 13')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  await waitFor(() => expect(screen.getByText('Worker option 13')).toBeVisible());
});

it('keeps technical profile JSON closed initially and blocks an invalid edit with its reason once', async () => {
  const user = userEvent.setup(); draw(<AgentScreen agentId={detailAgent.id} tab="profile" onTab={vi.fn()} />);
  await screen.findByRole('heading', { name: detailAgent.name });
  expect(screen.queryByRole('textbox', { name: i18n.t('agents.profile.json') })).not.toBeInTheDocument();
  await user.click(screen.getByText(i18n.t('common.details'), { exact: true }));
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: i18n.t('agents.profile.json') }) }));
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('agents.profile.json') }), { target: { value: 'null' } });
  expect(screen.getByRole('button', { name: i18n.t('agents.profile.save') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getAllByText(i18n.t('agents.profile.jsonInvalid'), { exact: true })).toHaveLength(1);
});

it.each(['en', 'zh-CN'] as const)('saves a host choice once, renders its accepted answer, and undoes only that accepted revision (%s)', async (locale) => {
  await setLocale(locale); const user = userEvent.setup();
  let accept: (host: typeof detailHost) => void = () => undefined;
  vi.mocked(updateHost).mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }));
  const view = draw(<HostScreen hostId={detailHost.id} tab="settings" onTab={vi.fn()} />);
  const choice = await screen.findByRole('combobox', { name: i18n.t('hosts.settings.fullTitle') });
  await user.click(choice); await user.click(screen.getByRole('option', { name: i18n.t('hosts.whenFull.block') }));
  await waitFor(() => expect(updateHost).toHaveBeenCalledExactlyOnceWith(detailHost.id, { expected_revision: 1, when_full: 'block' }));
  expect(choice).toBeDisabled();
  const accepted = { ...detailHost, when_full: { value: 'block' as const, own: true }, revision: 2 };
  await act(async () => { accept(accepted); await Promise.resolve(); });
  await waitFor(() => expect(choice).toBeEnabled()); expect(choice).toHaveTextContent(i18n.t('hosts.whenFull.block'));
  expect(view.client.getQueryData(hostKey(detailHost.id))).toEqual(accepted); expect(calls.read).toHaveBeenCalledExactlyOnceWith('host');
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('resourceDetails.layout.saved'));
  vi.mocked(updateHost).mockResolvedValueOnce({ ...detailHost, revision: 3 });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.undo') }));
  await waitFor(() => expect(updateHost).toHaveBeenLastCalledWith(detailHost.id, { expected_revision: 2, when_full: null }));
  await waitFor(() => expect(screen.queryByRole('button', { name: i18n.t('settings.compact.undo') })).not.toBeInTheDocument());
  expect(calls.read).toHaveBeenCalledTimes(1);
});

it('keeps a host name draft when the observed revision changes and blocks an unreviewed write', async () => {
  const user = userEvent.setup(), view = draw(<HostScreen hostId={detailHost.id} tab="settings" onTab={vi.fn()} />);
  const name = await screen.findByRole('textbox', { name: i18n.t('hosts.settings.name') });
  fireEvent.change(name, { target: { value: 'My host name' } });
  act(() => { view.client.setQueryData(hostKey(detailHost.id), { ...detailHost, name: 'Other host name', revision: 2 }); });
  const save = screen.getByRole('button', { name: i18n.t('hosts.settings.save') });
  await waitFor(() => expect(save).toHaveAttribute('aria-disabled', 'true')); expect(name).toHaveValue('My host name');
  await user.click(save); expect(updateHost).not.toHaveBeenCalled();
  await user.click(screen.getByText(i18n.t('settings.compact.review'), { exact: true }));
  expect(screen.getByRole('table', { name: i18n.t('settings.compact.review') })).toHaveTextContent('Other host name');
});

it.each(['en', 'zh-CN'] as const)('shows a host reader values under one authority sentence and no writable controls (%s)', async (locale) => {
  await setLocale(locale);
  const host = { ...detailHost, access: { ...detailHost.access, permissions: ['host.view'] } };
  const view = draw(<HostScreen hostId={host.id} tab="settings" onTab={vi.fn()} />, host);
  expect(await screen.findByText(i18n.t('hosts.whenFull.warn'), { exact: true })).toBeVisible();
  expect(view.container.querySelectorAll('[data-read-only-settings]')).toHaveLength(1);
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument(); expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('hosts.settings.save') })).not.toBeInTheDocument(); expect(updateHost).not.toHaveBeenCalled();
});
