/** Discovery fills the real locator once; refreshes never replace a person's profile draft. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useQueryClient, type QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import type * as Programs from '@/api/machinePrograms';
import { AddAgentDialog } from '@/features/resources/agents/AddAgent';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import type { HarnessKindInfo, Machine, MachineProgram, ProviderRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { harnessName, partialDescriptor } from '../doubles/harness-names';
import { detailAgent, detailMachine } from './resource-detail-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const calls = vi.hoisted(() => ({ kinds: vi.fn<() => Promise<HarnessKindInfo[]>>(), machine: vi.fn<() => Promise<Machine>>(),
  create: vi.fn<typeof Resources.createAgent>(), register: vi.fn<typeof Resources.registerAgent>(), program: vi.fn<typeof Programs.changeMachinePrograms>(), help: vi.fn() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), createAgent: calls.create, registerAgent: calls.register,
  harnessKindsQuery: { queryKey: ['profile-detection-kinds'], queryFn: calls.kinds },
  machinesQuery: { queryKey: ['profile-detection-machines'], queryFn: async () => [await calls.machine()] },
  machineQuery: (id: string) => ({ queryKey: ['profile-detection-machine', id], queryFn: calls.machine }) }));
vi.mock('@/api/machinePrograms', () => ({ changeMachinePrograms: calls.program, machineProgramsKey: (id: string) => ['profile-programs', id],
  machineProgramsQuery: (id: string) => ({ queryKey: ['profile-programs', id], queryFn: () => Promise.resolve({ entries: [], can_manage: true, config_file: null,
    capabilities: [{ kind: 'agent', supported: true, delegated: true, enabled: true, remote: true, policy_key: 'agent.programs', reason: null }] }) }) }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.help }));
vi.mock('@/features/resources/HomePicker', () => ({ HomePicker: ({ value, onChange }: Pick<HomePickerProps, 'value' | 'onChange'>) =>
  <select aria-label="Fixture home" value={value} onChange={(event) => onChange(event.target.value)}><option value="home">Workshop</option><option value="other-home">Other home</option></select> }));
const registry: HarnessKindInfo[] = ['codex', 'claude-code'].map((kind) => ({ kind, title: harnessName(kind).name, harness: harnessName(kind), builds: [], program: null,
  descriptor: partialDescriptor({ harness: { version: 'catalogue-99' }, arrangements: [{ type: 'separated' }, { type: 'together' }] }) }));
const account: ProviderRef = { id: 'reported-account', name: 'Existing subscription', address: 'existing', kind: 'subscription', harness_kind: 'codex', harness: harnessName('codex'), placement: 'connected', status: 'needs_setup' };
const custom: MachineProgram = { kind: 'agent', name: 'Lab adapter', command: ['lab-adapter'], origin: 'orch', ready: true, version: 'adapter-99',
  descriptor: { kind: 'lab-agent', harness: { name: 'Lab agent', version: 'harness-4' }, arrangements: [{ type: 'separated' }] }, models: null, detail: null };
let machine: Machine;
const cache: { current: QueryClient | null } = { current: null };
function CaptureCache() { cache.current = useQueryClient(); return null; }
function open() { show(<><CaptureCache /><AddAgentDialog initialHome="home" onClose={vi.fn()} onCreated={vi.fn()} /></>); }
async function chooseMachine(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('combobox', { name: i18n.t('agentCreation.mode') }));
  await user.click(screen.getByRole('option', { name: i18n.t('agentCreation.onMachine') }));
  await user.click(await screen.findByRole('combobox', { name: i18n.t('agentCreation.machine') }));
  await user.click(screen.getByRole('option', { name: machine.name }));
  await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('agents.add.harness') })).toHaveValue('Codex'));
}
async function refresh(next: Machine) {
  if (cache.current === null) throw new Error('the real profile query cache was not mounted');
  const client = cache.current;
  machine = next;
  await act(() => { client.setQueryData(['profile-detection-machine', machine.id], next); return Promise.resolve(); });
}
beforeEach(() => {
  vi.clearAllMocks(); cache.current = null;
  machine = { ...detailMachine, online: true, roles: ['agent'], providers: [account], facts: {
    adapters: [{ kind: 'codex', builtin: true }, { kind: 'claude-code', builtin: true }],
    harnesses: [{ kind: 'codex', path: '/program/codex', version: 'installed-2', daemon: false }, { kind: 'claude-code', path: '/program/claude', version: 'installed-3', daemon: false }],
    policy: { 'provider.arrangements': ['separated'] },
  } };
  calls.kinds.mockResolvedValue(registry); calls.machine.mockImplementation(() => Promise.resolve(machine));
  calls.create.mockResolvedValue(detailAgent); calls.register.mockResolvedValue(detailAgent);
  calls.program.mockResolvedValue({ entries: [custom], can_manage: true, config_file: null, capabilities: [] });
});
afterEach(async () => { document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) describe(`${locale}/${theme}`, () => {
  it('fills a confirmed program, labels actual observations, and preserves overrides through changed reports', async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme; const t = i18n.getFixedT(locale); const user = userEvent.setup(); open();
    expect(await screen.findByRole('combobox', { name: t('agents.add.harness') })).toHaveValue('');
    await chooseMachine(user);
    const facts = screen.getByLabelText(t('agentDetection.title'));
    expect(within(facts).getByText('installed-2')).toBeInTheDocument(); expect(within(facts).queryByText('catalogue-99')).not.toBeInTheDocument();
    expect(within(facts).getByText(account.name)).toBeInTheDocument(); expect(within(facts).getByText(t('providers.status.needs_setup'))).toBeInTheDocument();
    expect(within(facts).getByText(t('agentDetection.automatic'))).toBeInTheDocument();
    expect(calls.create).not.toHaveBeenCalled(); expect(calls.register).not.toHaveBeenCalled();
    await user.click(screen.getByRole('combobox', { name: t('agents.add.harness') })); await user.click(screen.getByRole('option', { name: 'Claude Code' }));
    await user.click(screen.getByLabelText(t('agents.add.name'))); await user.paste('My retained profile');
    await user.selectOptions(screen.getByRole('combobox', { name: 'Fixture home' }), 'other-home');
    await refresh({ ...machine, facts: { ...machine.facts as object, harnesses: [
      { kind: 'claude-code', path: '/program/claude', version: 'new-observation', daemon: false },
      { kind: 'codex', path: '/program/codex', version: 'new-codex', daemon: false },
    ] } });
    expect(screen.getByRole('combobox', { name: t('agents.add.harness') })).toHaveValue('Claude Code');
    await screen.findByText('new-observation');
    expect(screen.getByText(t('agentDetection.yourChoice'))).toBeInTheDocument();
    expect(screen.getByLabelText(t('agents.add.name'))).toHaveValue('My retained profile'); expect(screen.getByRole('combobox', { name: 'Fixture home' })).toHaveValue('other-home');
    await user.click(screen.getByRole('button', { name: t('agents.add.submit') }));
    await waitFor(() => expect(calls.register).toHaveBeenCalledExactlyOnceWith(machine.id, { home_id: 'other-home', name: 'My retained profile', harness_kind: 'claude-code' }, expect.any(String)));
  });
  it('keeps a disappeared selection named and blocks creation until another real program is chosen', async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme; const t = i18n.getFixedT(locale); const user = userEvent.setup(); open(); await chooseMachine(user);
    await user.click(screen.getByLabelText(t('agents.add.name'))); await user.paste('Keep this draft');
    await refresh({ ...machine, facts: { adapters: [{ kind: 'claude-code', builtin: true }], harnesses: [{ kind: 'claude-code', version: '', path: '/program/claude', daemon: false }] } });
    expect(screen.getByRole('combobox', { name: t('agents.add.harness') })).toHaveValue('Codex');
    await screen.findByText(t('agentDetection.unavailable'));
    await user.click(screen.getByRole('button', { name: t('agents.add.submit') })); expect(calls.register).not.toHaveBeenCalled();
    await user.click(screen.getByRole('combobox', { name: t('agents.add.harness') })); await user.click(screen.getByRole('option', { name: 'Claude Code' }));
    expect(screen.getByLabelText(t('agents.add.name'))).toHaveValue('Keep this draft');
    expect(screen.getByText(t('agents.overview.versionUnknown'))).toBeInTheDocument();
    expect(screen.getByText(t('agentDetection.accountsUnknown'))).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: t('agents.add.submit') }));
    await waitFor(() => expect(calls.register).toHaveBeenCalledExactlyOnceWith(machine.id, { home_id: 'home', name: 'Keep this draft', harness_kind: 'claude-code' }, expect.any(String)));
  });
  it('fills a sole catalogue choice without inventing an installed version, account or capability route', async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme; const t = i18n.getFixedT(locale); calls.kinds.mockResolvedValue([registry[0]!]); const user = userEvent.setup(); open();
    await waitFor(() => expect(screen.getByRole('combobox', { name: t('agents.add.harness') })).toHaveValue('Codex'));
    expect(screen.getAllByText(t('agentDetection.chooseMachine'))).toHaveLength(2); expect(screen.queryByText('catalogue-99')).not.toBeInTheDocument();
    expect(calls.machine).not.toHaveBeenCalled(); expect(calls.create).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: t('taskGuides.open', { title: t('taskGuides.registerProgram') }) })); expect(calls.help).toHaveBeenCalledExactlyOnceWith('register-your-program');
    await user.type(screen.getByLabelText(t('agents.add.name')), 'Available profile'); await user.click(screen.getByRole('button', { name: t('agents.add.submit') }));
    await waitFor(() => expect(calls.create).toHaveBeenCalledExactlyOnceWith('home', { name: 'Available profile', harness_kind: 'codex' }, expect.any(String)));
  });
  it('uses an inline declaration only until a newer machine revision confirms its actual current programs', async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme; const t = i18n.getFixedT(locale); const user = userEvent.setup(); open(); await chooseMachine(user);
    await user.click(screen.getByRole('button', { name: t('agentCreation.register') }));
    const registering = screen.getByRole('dialog', { name: t('machinePrograms.addTitle') });
    await user.type(await within(registering).findByRole('textbox', { name: t('machinePrograms.name') }), custom.name);
    await user.type(within(registering).getByRole('textbox', { name: t('machinePrograms.command') }), 'lab-adapter');
    await user.click(within(registering).getByRole('button', { name: t('machinePrograms.register') }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: t('agents.add.harness') })).toHaveValue('Lab agent'));
    expect(screen.getByText('harness-4')).toBeInTheDocument(); expect(screen.queryByText('adapter-99')).not.toBeInTheDocument();
    await user.type(screen.getByLabelText(t('agents.add.name')), 'Retain custom profile draft');
    await refresh({ ...machine, revision: machine.revision + 1 });
    expect(screen.getByRole('combobox', { name: t('agents.add.harness') })).toHaveValue('Lab agent');
    await screen.findByText(t('agentDetection.unavailable'));
    await user.click(screen.getByRole('button', { name: t('agents.add.submit') })); expect(calls.register).not.toHaveBeenCalled();
    await refresh({ ...machine, revision: machine.revision + 2, facts: {
      adapters: [{ kind: 'lab-agent', needs_binary: false, builtin: false }], harnesses: [],
      programs: { entries: [{ ...custom, descriptor: { kind: 'lab-agent', harness: { name: 'Lab agent', version: 'harness-5' }, arrangements: [] } }] },
    } });
    await screen.findByText('harness-5');
    expect(screen.queryByText(t('agentDetection.unavailable'))).not.toBeInTheDocument();
    expect(screen.queryByText('harness-4')).not.toBeInTheDocument();
    expect(screen.getByLabelText(t('agents.add.name'))).toHaveValue('Retain custom profile draft');
  });
});


it.each(['en', 'zh-CN'] as const)('names the new agent profile before its setup choices in %s', async (locale) => {
  await setLocale(locale);
  open();
  const name = screen.getByRole('textbox', { name: i18n.t('agents.add.name') });
  expect(screen.getByRole('dialog').querySelector('input:not([type="hidden"]),textarea,[role="combobox"],select')).toBe(name);
});
