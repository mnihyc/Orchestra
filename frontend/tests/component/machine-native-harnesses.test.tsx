/** The actual Programs tab confirms reported binaries without changing the program registry. */
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { machineKey } from '@/api/resources';
import { ApiError } from '@/api/client';
import { MachineProgramsTab } from '@/features/resources/machines/Programs';
import type { Machine, MachinePrograms, MachineProgramsCommand } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { detailMachine } from './resource-detail-fixtures';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<MachinePrograms>>(), write: vi.fn<(id: string, body: MachineProgramsCommand) => Promise<MachinePrograms>>() }));
vi.mock('@/api/machinePrograms', () => ({ machineProgramsKey: (id: string) => ['machine-programs', id], machineProgramsQuery: (id: string) => ({ queryKey: ['machine-programs', id], queryFn: calls.read }), changeMachinePrograms: calls.write }));
vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => (kind: string) => kind === 'codex' ? 'Codex' : kind === 'claude-code' ? 'Claude Code' : 'Other agent' }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/machines/Enrollment', () => ({ AddMachineDialog: () => null }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="/resources/machines/machine?tab=policy">{children}</a> }));

const program = { kind: 'codex', path: '/opt/agent bins/codex', version: '0.164.0', daemon: true };
const command = '"/opt/Orch Connector" --data "/var/instance" policy';
function machine(facts: Record<string, unknown>): Machine {
  return { ...detailMachine, id: 'machine', roles: ['environment'], facts,
    installation: { program: 'orch-connector', executable: '/opt/Orch Connector', config_file: '/var/instance/config.toml', data_dir: '/var/instance', user: 'operator', system: false, service: null,
      commands: { policy: command, roles: null, start: 'actual start', stop: 'actual stop', update: 'actual update', uninstall: null } } };
}
const policy = { 'agent.harnesses': [], 'policy.delegated': ['agent.harnesses'] };
const confirmedPolicy = { ...policy, 'agent.harnesses': [{ kind: program.kind, path: program.path }] };
function snapshot(canManage = true, view?: Machine): MachinePrograms { return { entries: [], capabilities: [], config_file: '/var/instance/config.toml', can_manage: canManage, ...(view === undefined ? {} : { machine_view: view }) }; }
function draw(initial: Machine) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  function Host() { const row = useQuery({ queryKey: machineKey(initial.id), initialData: initial, enabled: false }); return <MachineProgramsTab machine={row.data} />; }
  return { cache, ...render(<QueryClientProvider client={cache}><TooltipProvider><Toaster><Host /></Toaster></TooltipProvider></QueryClientProvider>) };
}
beforeEach(() => { vi.clearAllMocks(); calls.read.mockResolvedValue(snapshot()); calls.write.mockResolvedValue(snapshot()); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  const label = locale === 'en' ? { use: 'Use', confirmed: 'Confirmed', remove: 'Remove', refresh: 'Refresh programs', unknown: 'Version unknown' }
    : { use: '使用', confirmed: '已确认', remove: '移除', refresh: '刷新程序', unknown: '版本未知' };
  it(`uses only the found kind/path and applies the acknowledged machine without another read (${locale})`, async () => {
    await setLocale(locale);
    const initial = machine({ harnesses: [], discovered_harnesses: [program], searched_harness_kinds: ['codex'], policy });
    const updated = machine({ harnesses: [program], discovered_harnesses: [], searched_harness_kinds: ['codex'], policy: confirmedPolicy });
    calls.write.mockResolvedValue(snapshot(true, updated));
    const user = userEvent.setup(); const view = draw(initial);
    const use = await screen.findByRole('button', { name: label.use });
    expect(screen.getByText('…/agent bins/codex')).toBeVisible(); expect(screen.getByText(program.version)).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.copyPath') }));
    expect(await navigator.clipboard.readText()).toBe(program.path);
    await user.click(use);
    await screen.findByText(label.confirmed, { exact: true });
    expect(calls.write).toHaveBeenCalledExactlyOnceWith('machine', { action: 'confirm_harness', kind: program.kind, path: program.path });
    expect(view.cache.getQueryData(machineKey('machine'))).toEqual(updated);
    expect(calls.read).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: label.use })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: label.remove })).toBeEnabled();
  });
  it(`removes a confirmed tuple through its own action and preserves Refresh (${locale})`, async () => {
    await setLocale(locale);
    const initial = machine({ harnesses: [program], discovered_harnesses: [], searched_harness_kinds: ['codex'], policy: confirmedPolicy });
    calls.write.mockResolvedValue(snapshot(true, machine({ harnesses: [], discovered_harnesses: [program], searched_harness_kinds: ['codex'], policy })));
    const user = userEvent.setup(); draw(initial);
    const remove = await screen.findByRole('button', { name: label.remove });
    await user.click(remove);
    const confirm = screen.queryByRole('alertdialog');
    if (confirm !== null) await user.click(within(confirm).getByRole('button', { name: label.remove }));
    const use = await screen.findByRole('button', { name: label.use });
    await waitFor(() => expect(use).toHaveFocus());
    expect(calls.write).toHaveBeenCalledExactlyOnceWith('machine', { action: 'remove_harness', kind: program.kind, path: program.path });
    expect(screen.getByRole('button', { name: label.refresh })).toBeEnabled();
    expect(calls.read).toHaveBeenCalledTimes(1);
  });
  it(`shows unknown versions and keeps reader actions absent (${locale})`, async () => {
    await setLocale(locale); calls.read.mockResolvedValue(snapshot(false));
    draw(machine({ harnesses: [{ ...program, version: 'unknown' }], discovered_harnesses: [], searched_harness_kinds: ['codex'], policy: confirmedPolicy }));
    await screen.findByText(label.unknown, { exact: true });
    expect(screen.getByText('…/agent bins/codex')).toBeVisible();
    for (const name of [label.use, label.remove, label.refresh]) expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
    expect(calls.write).not.toHaveBeenCalled();
  });
}

it('keeps a different canonical path distinct when the acknowledged discovery replaces a removed tuple', async () => {
  const initial = machine({ harnesses: [program], discovered_harnesses: [], searched_harness_kinds: ['codex'], policy: confirmedPolicy });
  const replacement = { ...program, path: '/opt/another/codex' };
  calls.write.mockResolvedValue(snapshot(true, machine({ harnesses: [], discovered_harnesses: [replacement], searched_harness_kinds: ['codex'], policy })));
  const user = userEvent.setup(); draw(initial);
  const remove = await screen.findByRole('button', { name: 'Remove' }); await user.click(remove);
  await user.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Remove' }));
  const use = await screen.findByRole('button', { name: 'Use' });
  expect(use).not.toBe(remove); expect(remove.isConnected).toBe(false);
  expect(screen.getByText('…/another/codex')).toBeVisible();
  expect(calls.write).toHaveBeenCalledExactlyOnceWith('machine', { action: 'remove_harness', kind: program.kind, path: program.path });
});

it('keeps local harness policy exact even when the separate registry is delegated', async () => {
  const user = userEvent.setup();
  draw(machine({ harnesses: [], discovered_harnesses: [program], searched_harness_kinds: ['codex'], policy: { 'agent.harnesses': [{ kind: 'claude-code', path: '/opt/claude' }], 'policy.delegated': ['agent.programs'] } }));
  await screen.findByText('…/agent bins/codex');
  expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
  const exact = `${command} set agent.harnesses '[{"kind":"claude-code","path":"/opt/claude"}]'`;
  expect(screen.getByText((_text, element) => element?.tagName === 'CODE' && element.textContent === exact)).toBeVisible();
  const refresh = screen.getByRole('button', { name: 'Refresh programs' }); await user.click(refresh);
  await waitFor(() => expect(calls.write).toHaveBeenCalledExactlyOnceWith('machine', { action: 'reload' }));
});

it('keeps Use and Remove absent when a local harness value vetoes wildcard handover', async () => {
  draw(machine({ harnesses: [program], discovered_harnesses: [{ ...program, kind: 'claude-code', path: '/other/claude' }], searched_harness_kinds: ['codex', 'claude-code'],
    policy: { ...confirmedPolicy, 'policy.delegated': ['*'], 'policy.local': { 'agent.harnesses': confirmedPolicy['agent.harnesses'] } } }));
  await screen.findByText('Confirmed', { exact: true });
  for (const name of ['Use', 'Remove']) expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Refresh programs' })).toBeEnabled();
  expect(calls.write).not.toHaveBeenCalled();
});

it('does not offer native mutation for a machine without an applicable role', async () => {
  draw({ ...machine({ harnesses: [], discovered_harnesses: [program], searched_harness_kinds: ['codex'], policy }), roles: ['provider'] });
  await screen.findByText('Found', { exact: true });
  expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
  expect(screen.getByText('Agent programs need the Environment or Agent role.')).toBeVisible();
  expect(calls.write).not.toHaveBeenCalled();
});

it('shows the actual local policy command after a current native refusal', async () => {
  calls.write.mockRejectedValue(new ApiError({ type: 'orch:error:policy.not_delegated', code: 'policy.not_delegated', status: 403,
    title: 'Kept on this machine', detail: 'The setting is kept locally.', request_id: 'native-program-policy' }, null));
  const user = userEvent.setup();
  draw(machine({ harnesses: [program], discovered_harnesses: [], searched_harness_kinds: ['codex'], policy: confirmedPolicy }));
  await user.click(await screen.findByRole('button', { name: 'Remove' }));
  await user.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Remove' }));
  await screen.findByText('Agent programs are kept in this machine’s local policy.');
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Remove' })).not.toBeInTheDocument();
  const exact = `${command} set agent.harnesses '${JSON.stringify(confirmedPolicy['agent.harnesses'])}'`;
  expect(screen.getByText((_text, element) => element?.tagName === 'CODE' && element.textContent === exact)).toBeVisible();
  expect(calls.write).toHaveBeenCalledExactlyOnceWith('machine', { action: 'remove_harness', kind: program.kind, path: program.path });
  expect(calls.read).toHaveBeenCalledTimes(1);
});

it('does not call an unreported policy a local refusal', async () => {
  draw(machine({}));
  await screen.findByText('This machine has not reported agent program discovery.');
  expect(screen.getByText('This machine has not reported its current harness policy and command.')).toBeVisible();
  expect(screen.queryByText('Agent programs are kept in this machine’s local policy.')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
  expect(calls.write).not.toHaveBeenCalled();
});
