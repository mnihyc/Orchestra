/** A selected Programs view reuses only its current Machine projection and retains exact read repairs. */
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { applyChanges, resourceChanges } from '@/api/acts';
import { machineKey } from '@/api/resources';
import { queryClient } from '@/api/query';
import { MachineProgramsTab } from '@/features/resources/machines/Programs';
import type { Machine, MachinePrograms } from '@/generated/api-types';
import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { detailAccess, detailMachine } from './resource-detail-fixtures';

const read = vi.hoisted(() => vi.fn<() => Promise<MachinePrograms>>());
vi.mock('@/api/machinePrograms', () => ({ machineProgramsKey: (id: string) => ['machine-programs', id], machineProgramsQuery: (id: string) => ({ queryKey: ['machine-programs', id], queryFn: read }), changeMachinePrograms: vi.fn() }));
vi.mock('@/features/resources/harnesses', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHarnessTitle: () => () => 'Codex' }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/machines/Enrollment', () => ({ AddMachineDialog: () => null }));
vi.mock('@tanstack/react-router', async (loadOriginal) => ({ ...await loadOriginal<object>(), Link: ({ children }: { children: ReactNode }) => <a href="#machine">{children}</a> }));

const binary = { kind: 'codex', path: '/usr/local/bin/codex', version: '0.164.0', daemon: true };
const report = (flag = false): MachinePrograms => ({ entries: [], capabilities: [], config_file: '/instance/config.toml', can_manage: flag });
function machine(manages: boolean, programs: unknown = report(!manages)): Machine {
  return { ...detailMachine, id: 'projection-machine', roles: ['environment'],
    access: { ...detailAccess, permissions: manages ? ['machine.view', 'machine.use', 'machine.manage'] : ['machine.view'], through: [] },
    facts: { programs, harnesses: [], discovered_harnesses: [binary], searched_harness_kinds: ['codex'], policy: { 'agent.harnesses': [], 'policy.delegated': ['agent.harnesses'] } } };
}
function draw(initial: Machine, current = true, old = false) {
  const defaults = queryClient.getDefaultOptions();
  const cache = new QueryClient({ defaultOptions: { ...defaults, queries: { ...defaults.queries, retry: false }, mutations: { ...defaults.mutations, retry: false } } });
  if (current) cache.setQueryData(machineKey(initial.id), initial, old ? { updatedAt: Date.now() - 60_000 } : undefined);
  function Host() {
    const row = useQuery({ queryKey: machineKey(initial.id), enabled: false, queryFn: () => Promise.resolve(initial) });
    return row.data === undefined ? <MachineProgramsTab machine={initial} /> : <MachineProgramsTab machine={row.data} />;
  }
  return { cache, ...render(<QueryClientProvider client={cache}><TooltipProvider><Toaster><Host /></Toaster></TooltipProvider></QueryClientProvider>) };
}
beforeEach(() => { read.mockReset(); read.mockResolvedValue(report(false)); });
afterEach(() => { cleanup(); });

it('uses a fresh admitted Machine report with its current Manage instead of another Programs GET', async () => {
  read.mockResolvedValue(report(true)); draw(machine(true));
  expect(await screen.findByRole('button', { name: 'Use' })).toBeEnabled();
  expect(read).not.toHaveBeenCalled();
});

it('does not take Manage from a native report when the current API row is read-only', async () => {
  draw(machine(false));
  expect(await screen.findByText('Found', { exact: true })).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument();
  expect(read).not.toHaveBeenCalled();
});

it('keeps the existing GET for absent or malformed reports rather than inventing an empty report', async () => {
  for (const programs of [null, { entries: 'not-an-array', capabilities: [], config_file: null, can_manage: false }]) {
    const value = machine(false, programs), view = draw(value);
    await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    view.unmount(); read.mockClear();
  }
});

it('does not seed from a detached or retired Machine object outside its current query cache', async () => {
  draw(machine(false), false);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
});

it('retains the Machine snapshot age so an old report still receives the normal read', async () => {
  draw(machine(false), true, true);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
});

it('lowers an already-open projection using the same full native Machine row without a read', async () => {
  const initial = machine(true); read.mockResolvedValue(report(true)); const view = draw(initial);
  await screen.findByRole('button', { name: 'Use' });
  const lowered = machine(false);
  await act(async () => { await applyChanges(resourceChanges('machine', initial.id, { what: 'changed', row: { kind: 'machine', value: lowered } }), view.cache); });
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument());
  expect(view.cache.getQueryData<MachinePrograms>(['machine-programs', initial.id])?.can_manage).toBe(false);
  expect(read).not.toHaveBeenCalled();
});

it('withdraws a malformed live report and repairs only that exact Programs key', async () => {
  const initial = machine(true); read.mockResolvedValue(report(true)); const view = draw(initial);
  expect(await screen.findByRole('button', { name: 'Use' })).toBeVisible();
  read.mockClear();
  read.mockResolvedValue(report(false));
  const other = vi.fn(() => Promise.resolve('unrelated')); view.cache.setQueryData(['unrelated-resource'], 'kept');
  view.cache.setQueryDefaults(['unrelated-resource'], { queryFn: other });
  const invalid = machine(false, { entries: 'invalid' });
  await act(async () => { await applyChanges(resourceChanges('machine', initial.id, { what: 'changed', row: { kind: 'machine', value: invalid } }), view.cache); });
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument());
  expect(view.cache.getQueryData(['unrelated-resource'])).toBe('kept'); expect(other).not.toHaveBeenCalled();
});

it('retires the open Programs data together with a revoked Machine view', async () => {
  const initial = machine(true); read.mockResolvedValue(report(true)); const view = draw(initial);
  await screen.findByRole('button', { name: 'Use' });
  await act(async () => { await applyChanges(resourceChanges('machine', initial.id, { what: 'changed', view_revoked: true }), view.cache); });
  expect(view.cache.getQueryData(['machine-programs', initial.id])).toBeUndefined();
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Use' })).not.toBeInTheDocument());
  expect(read).not.toHaveBeenCalled();
});
