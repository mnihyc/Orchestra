/** Complete adapter/build/facts metadata searches locally; only each caller's deliberate action writes native IDs. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Admin from '@/api/admin';
import type * as Resources from '@/api/resources';
import { ApiError } from '@/api/client';
import { Agents } from '@/features/admin/Agents';
import { AgentList, type AgentsSearch } from '@/features/resources/agents/AgentList';
import { EditAgent } from '@/features/resources/agents/Edit';
import { AgentScreen } from '@/features/resources/agents/Profile';
import { harnessVersionWords } from '@/features/resources/agents/descriptor';
import { RegisterAgentDialog } from '@/features/resources/machines/RegisterAgent';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import { hintOf } from '@/features/help/hints';
import type { Agent, HarnessKindInfo, Machine } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';
import { Toaster } from '@/ui/Toast';
import { id } from '../doubles/fixtures';
import { detailAgent, detailMachine } from './resource-detail-fixtures';
import { agentDescriptor } from '../doubles/agent-descriptor';
import { harnessName } from '../doubles/harness-names';

const calls = vi.hoisted(() => ({ kinds: vi.fn<() => Promise<HarnessKindInfo[]>>(), agent: vi.fn<() => Promise<Agent>>(),
  make: vi.fn<typeof Admin.createSystemAgent>(), update: vi.fn<typeof Resources.updateAgent>(), register: vi.fn<typeof Resources.registerAgent>(), search: vi.fn(), close: vi.fn(), navigate: vi.fn() }));
const KINDS = ['metadata-locator-kinds'] as const;
const HOME_ID = id(70, 1), AGENT = id(70, 2), MACHINE = id(70, 3);
const versions = Array.from({ length: 13 }, (_, index) => ({ version: `${index + 1}.0.0`, is_default: index === 0 }));
const registry: HarnessKindInfo[] = Array.from({ length: 13 }, (_, index) => {
  const kind = index === 12 ? 'any' : `adapter-${String(index + 1).padStart(2, '0')}`;
  const name = index === 12 ? 'Adapter named any' : `Registry adapter ${index + 1}`;
  return { kind, title: name, builds: versions, program: null,
    harness: { kind, name, names: {} }, descriptor: agentDescriptor(name, { kind }) };
});
const agent: Agent = { ...detailAgent, id: AGENT, home: { id: HOME_ID, kind: 'org', name: 'Workshop', redacted: false }, org_id: HOME_ID,
  harness_kind: 'adapter-01', profile: { harness_version: 'retained-build', keep: 'native-profile-member' }, revision: 7 };

vi.mock('@/api/admin', async (load) => ({ ...await load<typeof Admin>(), createSystemAgent: calls.make,
  harnessKindsQuery: { queryKey: ['metadata-locator-kinds'], queryFn: calls.kinds },
  systemAgentsQuery: { queryKey: ['metadata-system-agents'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), updateAgent: calls.update, registerAgent: calls.register,
  harnessKindsQuery: { queryKey: ['metadata-locator-kinds'], queryFn: calls.kinds },
  agentQuery: (key: string) => ({ queryKey: ['agent', key], queryFn: calls.agent }) }));
vi.mock('@/api/executors', () => ({ executorCatalogueQuery: { queryKey: ['profile-executors'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => calls.navigate,
  Link: ({ children, to }: { children: ReactNode; to: string }) => <a href={to}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => null }));
vi.mock('@/features/navigation/placeLabels', () => ({ useKnownPlaces: () => new Map() }));
vi.mock('@/features/resources/agents/acts', () => ({ useAgentActs: () => ({ list: [], dialogs: null, page: () => ({ edit: null, primary: null, menu: [] }) }) }));
vi.mock('@/features/resources/agents/Overrides', () => ({ DescriptorOverrides: () => null }));
vi.mock('@/features/resources/ResourceList', () => ({ ResourceList: ({ filters }: { filters: ReactNode }) => <section>{filters}</section>, useSearchText: (value: string) => [value, vi.fn()] }));
vi.mock('@/features/resources/DetailSections', async (load) => ({ ...await load<object>(),
  ResourceTabs: ({ value, tabs }: { value: string; tabs: { value: string; panel: ReactNode }[] }) => <>{tabs.find((tab) => tab.value === value)?.panel}</>,
  ResourceDetails: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title }: { title: string }) => <h1>{title}</h1> }));
vi.mock('@/features/resources/HomePicker', () => ({ HomePicker: ({ value, onChange, disabled }: Pick<HomePickerProps, 'value' | 'onChange' | 'disabled'>) =>
  <select aria-label="Fixture home" value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)}><option value="">Choose</option><option value={HOME_ID}>Workshop</option></select> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/help', async (load) => ({ ...await load<object>(), useGuide: () => ({ articles: [] }) }));

function draw(ui: ReactNode, knownKinds?: HarnessKindInfo[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  if (knownKinds !== undefined) {
    client.setQueryData(KINDS, knownKinds);
    void client.invalidateQueries({ queryKey: KINDS, refetchType: 'none' });
  }
  const view = render(<QueryClientProvider client={client}><TooltipProvider><Toaster>{ui}</Toaster></TooltipProvider></QueryClientProvider>);
  return { ...view, client };
}

beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks(); calls.update.mockReset();
  calls.kinds.mockResolvedValue(registry); calls.agent.mockResolvedValue(agent);
  calls.make.mockResolvedValue({ ...agent, name: 'System profile', home: null, org_id: null, harness_kind: 'adapter-12', profile: {}, revision: 1 });
  calls.update.mockImplementation((_id, body) => Promise.resolve({ ...agent, revision: 8, name: body.name ?? agent.name,
    profile: body.profile ?? agent.profile, disabled: body.disabled ?? agent.disabled }));
  calls.register.mockResolvedValue({ ...agent, name: 'Machine profile', harness_kind: 'any', profile: {}, revision: 1,
    machine: { id: MACHINE, name: detailMachine.name, online: true, placement: 'connected', platform_os: 'linux' } });
});
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`shows a profile's declared executor and tool set (${locale})`, async () => {
  await setLocale(locale);
  const declared = { kind: 'research-exec', title: { en: 'Research executor', 'zh-CN': '研究执行器' }, program: 'adapter-01', offers: [],
    tool_sets: [{ name: 'research-tools', title: { en: 'Research tools', 'zh-CN': '研究工具' }, summary: { en: 'Tools for research', 'zh-CN': '研究工具集' } }] };
  const program = { builtin: false, vendor: null, protocol: 'acp', sources: { channels: [], exact_fetch: false },
    checks: { cases: [], features: [], runner: { en: 'Check report', 'zh-CN': '检查报告' } }, carry: 'handoff' as const, executors: [declared],
    files: { instruction_files: [], workspace_config: [] }, colocated: true, privacy: [] };
  const named = { kind: 'adapter-01', title: 'Research assistant', harness: { kind: 'adapter-01', name: 'Research assistant', names: { 'zh-CN': '研究助理' } }, builds: [], program, descriptor: agentDescriptor('Research assistant', { kind: 'adapter-01' }) };
  calls.kinds.mockResolvedValue([named]);
  calls.agent.mockResolvedValue({ ...agent, descriptor: { harness: { name: 'Research assistant' }, arrangements: [{ type: 'separated', executor: 'research-exec', tool_sets: ['research-tools'] }] } });
  draw(<AgentScreen agentId={AGENT} tab="overview" onTab={vi.fn()} />);
  expect(await screen.findByText(i18n.t('agents.overview.executor', { executor: locale === 'zh-CN' ? '研究执行器' : 'Research executor' }))).toBeVisible();
  expect(screen.getByText(i18n.t('agents.overview.toolSets', { list: locale === 'zh-CN' ? '研究工具' : 'Research tools' }))).toBeVisible();
  expect(document.body).not.toHaveTextContent('research-exec');
  expect(document.body).not.toHaveTextContent('research-tools');
});

it('searches all system adapters without changing the profile or creating one until Make', async () => {
  const user = userEvent.setup(); draw(<Agents />);
  await user.click(await screen.findByRole('button', { name: i18n.t('admin.agents.newTitle') }));
  const field = await screen.findByRole('combobox', { name: i18n.t('admin.agents.adapter') });
  await waitFor(() => expect(field).toHaveValue('Registry adapter 1'));
  expect(field).toHaveAttribute('placeholder', i18n.t('ui.combobox.search'));
  await user.click(field); expect(screen.getAllByRole('option')).toHaveLength(13);
  await user.type(field, 'adapter-12'); expect(screen.getByRole('option', { name: 'Registry adapter 12' })).toBeVisible();
  expect(calls.kinds).toHaveBeenCalledOnce(); expect(calls.make).not.toHaveBeenCalled();
  await user.keyboard('{Escape}'); expect(field).toHaveValue('Registry adapter 1');
  await user.type(screen.getByRole('textbox', { name: i18n.t('admin.agents.name') }), 'System profile');
  await user.click(field); await user.type(field, 'adapter-12'); await user.keyboard('{Enter}');
  expect(field).toHaveValue('Registry adapter 12'); expect(calls.make).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('admin.agents.make') }));
  await waitFor(() => expect(calls.make).toHaveBeenCalledExactlyOnceWith({ name: 'System profile', harness_kind: 'adapter-12', profile: {} }));
  expect(calls.kinds).toHaveBeenCalledOnce();
});

it('keeps a missing current harness and distinguishes the Any filter from an adapter literally named any', async () => {
  function Filters() {
    const [search, setSearch] = useState<AgentsSearch>({ harness: 'removed-adapter' });
    return <AgentList search={search} onSearch={(patch) => { calls.search(patch); setSearch((before) => ({ ...before, ...patch })); }} />;
  }
  const user = userEvent.setup(); draw(<Filters />);
  const field = await screen.findByRole('combobox', { name: i18n.t('agents.list.harnessFilter') });
  await waitFor(() => expect(calls.kinds).toHaveBeenCalledOnce());
  await user.click(field); await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(15));
  await user.type(field, 'removed-adapter');
  expect(screen.getByRole('option')).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByText(i18n.t('agents.overview.notReported'), { exact: true })).toBeVisible();
  await user.keyboard('{Enter}'); expect(calls.search).not.toHaveBeenCalled();
  await user.clear(field); await user.type(field, 'Adapter named any'); await user.keyboard('{Enter}');
  expect(calls.search).toHaveBeenLastCalledWith({ harness: 'any' }); expect(field).toHaveValue('Adapter named any');
  await user.click(field); await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.statusFilter.any.name') }));
  expect(calls.search).toHaveBeenLastCalledWith({ harness: undefined });
  expect(field).toHaveValue(i18n.t('choiceCatalog.statusFilter.any.name')); expect(calls.kinds).toHaveBeenCalledOnce();
});

it('keeps the clear action reachable when an active harness filter is absent from a one-kind registry', async () => {
  calls.kinds.mockResolvedValue(registry.slice(0, 1));
  const user = userEvent.setup(); draw(<AgentList search={{ harness: 'removed-adapter' }} onSearch={calls.search} />);
  const field = await screen.findByRole('combobox', { name: i18n.t('agents.list.harnessFilter') });
  await waitFor(() => expect(calls.kinds).toHaveBeenCalledOnce());
  await user.click(field); await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(3));
  await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.statusFilter.any.name') }));
  expect(calls.search).toHaveBeenCalledExactlyOnceWith({ harness: undefined });
});

it('preserves the previously reported harness and clear action through a pending/refused registry without claiming absence or reading on typing', async () => {
  let reject: (error: unknown) => void = () => undefined;
  calls.kinds.mockReturnValueOnce(new Promise<HarnessKindInfo[]>((_resolve, refuse) => { reject = refuse; }));
  function Filters() {
    const [search, setSearch] = useState<AgentsSearch>({ harness: 'codex' });
    return <AgentList search={search} onSearch={(patch) => { calls.search(patch); setSearch((before) => ({ ...before, ...patch })); }} />;
  }
  const user = userEvent.setup(); draw(<Filters />, [{ kind: 'codex', title: 'Codex', builds: [], program: null,
    harness: harnessName('codex'), descriptor: agentDescriptor('Codex', { kind: 'codex' }) }]);
  const field = screen.getByRole('combobox', { name: i18n.t('agents.list.harnessFilter') });
  expect(field).toHaveValue('Codex'); expect(field).toHaveAccessibleDescription(i18n.t('common.loading'));
  expect(screen.queryByText(i18n.t('agents.overview.notReported'))).toBeNull();
  await user.click(field); await user.type(field, 'codex');
  expect(screen.getByRole('option', { name: 'Codex' })).not.toHaveAttribute('aria-disabled', 'true');
  await user.keyboard('{Enter}'); expect(calls.search).not.toHaveBeenCalled(); expect(calls.kinds).toHaveBeenCalledOnce();
  await act(async () => { reject(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403,
    title: 'Private registry diagnostic', detail: 'Private registry diagnostic', request_id: 'metadata-registry' }, null)); await Promise.resolve(); });
  await screen.findByRole('alert'); expect(field).toHaveValue('Codex');
  expect(screen.queryByText(i18n.t('agents.overview.notReported'))).toBeNull(); expect(screen.queryByText('Private registry diagnostic')).toBeNull();
  await user.click(field); await user.type(field, 'codex');
  expect(screen.getByRole('option', { name: 'Codex' })).not.toHaveAttribute('aria-disabled', 'true');
  await user.keyboard('{Escape}'); expect(field).toHaveValue('Codex'); expect(calls.kinds).toHaveBeenCalledOnce();
  await user.click(field); await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.statusFilter.any.name') }));
  expect(calls.search).toHaveBeenCalledExactlyOnceWith({ harness: undefined }); expect(calls.kinds).toHaveBeenCalledOnce();
});

it('retains a missing edit-build draft across registry refresh and saves Default as null with its original revision/profile members', async () => {
  let accept: (value: Agent) => void = () => undefined;
  calls.update.mockReturnValueOnce(new Promise<Agent>((resolve) => { accept = resolve; }));
  const user = userEvent.setup(), view = draw(<EditAgent agent={agent} onProfile={vi.fn()} onClose={calls.close} />);
  const field = await screen.findByRole('combobox', { name: i18n.t('agents.profile.pin') });
  await waitFor(() => expect(calls.kinds).toHaveBeenCalledOnce()); expect(field).toHaveValue('retained-build');
  const hint = hintOf('agent-pin', i18n.t);
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: hint.title }) }));
  expect(screen.getByText(hint.what, { exact: true })).toBeVisible(); await user.keyboard('{Escape}');
  await user.click(field); expect(screen.getAllByRole('option')).toHaveLength(15);
  await user.type(field, '13.0.0'); await user.keyboard('{Enter}');
  expect(field).toHaveValue('13.0.0'); expect(calls.update).not.toHaveBeenCalled();
  act(() => { view.client.setQueryData(KINDS, registry.map((kind) => ({ ...kind, builds: versions.slice(0, 12) }))); });
  await waitFor(() => expect(field).toHaveValue('13.0.0'));
  await user.click(field); await user.type(field, '13.0.0'); expect(screen.getByRole('option', { name: '13.0.0' })).toBeVisible();
  await user.keyboard('{Escape}'); expect(calls.update).not.toHaveBeenCalled();
  await user.click(field); await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.sourceKind.default.name') }));
  expect(calls.update).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
  await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(AGENT, { expected_revision: 7, profile: { harness_version: null, keep: 'native-profile-member' } }));
  expect(field).toBeDisabled();
  await act(async () => { accept({ ...agent, revision: 8, profile: { harness_version: null, keep: 'native-profile-member' } }); await Promise.resolve(); });
  await waitFor(() => expect(calls.close).toHaveBeenCalledOnce());
  expect(calls.kinds).toHaveBeenCalledOnce();
});

it('shows a profile JSON draft’s unavailable pin, keeps invalid JSON locked, and writes only after Save', async () => {
  const user = userEvent.setup(); draw(<AgentScreen agentId={AGENT} tab="profile" onTab={vi.fn()} />);
  const field = await screen.findByRole('combobox', { name: i18n.t('agents.profile.pin') });
  expect(field).toHaveValue(harnessVersionWords(i18n.t, 'retained-build'));
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: i18n.t('agents.profile.json') }) }));
  const json = screen.getByRole('textbox', { name: i18n.t('agents.profile.json') });
  fireEvent.change(json, { target: { value: '{' } }); expect(field).toBeDisabled();
  fireEvent.change(json, { target: { value: JSON.stringify({ harness_version: 'draft-build', keep: 'native-profile-member' }) } });
  expect(field).toBeEnabled(); expect(field).toHaveValue(harnessVersionWords(i18n.t, 'draft-build'));
  await user.click(field); expect(screen.getAllByRole('option')).toHaveLength(15);
  await user.type(field, '13.0.0'); await user.keyboard('{Enter}');
  expect(calls.update).not.toHaveBeenCalled(); expect(calls.kinds).toHaveBeenCalledOnce();
  await user.click(screen.getByRole('button', { name: i18n.t('agents.profile.save') }));
  await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(AGENT, { expected_revision: 7, profile: { harness_version: '13.0.0', keep: 'native-profile-member' } }));
});

it('registers a late reported machine harness on the unchanged physical machine/home only after Submit', async () => {
  const machine: Machine = { ...detailMachine, id: MACHINE, placement: 'connected', facts: {
    adapters: registry.map((kind) => ({ kind: kind.kind, builtin: false, needs_binary: true })),
    harnesses: registry.map((kind, index) => ({ kind: kind.kind, path: `/opt/agent-${index + 1}`, version: `${index + 1}.0.0`, daemon: false })),
  } };
  const user = userEvent.setup(); draw(<RegisterAgentDialog machine={machine} open onClose={calls.close} />);
  const field = await screen.findByRole('combobox', { name: i18n.t('machines.agent.harness') });
  await waitFor(() => expect(calls.kinds).toHaveBeenCalledOnce());
  expect(field).toHaveAccessibleDescription(i18n.t('machines.agent.harnessHint'));
  expect(field).toHaveAttribute('placeholder', i18n.t('ui.combobox.search'));
  await user.click(field); expect(within(screen.getByRole('listbox')).getAllByRole('option')).toHaveLength(13);
  await user.type(field, '/opt/agent-13');
  expect(screen.getByText('/opt/agent-13', { exact: true })).toBeVisible();
  await user.keyboard('{Enter}'); expect(calls.register).not.toHaveBeenCalled();
  await user.selectOptions(screen.getByRole('combobox', { name: 'Fixture home' }), HOME_ID);
  await user.type(screen.getByRole('textbox', { name: new RegExp(i18n.t('machines.agent.name')) }), 'Machine profile');
  await user.click(screen.getByRole('button', { name: i18n.t('machines.agent.submit') }));
  await waitFor(() => expect(calls.register).toHaveBeenCalledWith(MACHINE, { home_id: HOME_ID, harness_kind: 'any', name: 'Machine profile' }, expect.any(String)));
  expect(calls.register).toHaveBeenCalledOnce(); expect(calls.kinds).toHaveBeenCalledOnce();
});
