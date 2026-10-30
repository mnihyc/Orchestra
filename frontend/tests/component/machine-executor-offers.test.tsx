/** Policy offers use this machine's declarations, retaining aliases and missing selections safely. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { MachinePolicyTab } from '@/features/resources/machines/Policy';
import { DETAIL_PAGE_ROWS } from '@/features/resources/DetailSections';
import type { MachineExecutor, MachinePolicy } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { show } from './support';

const { read, write, executors } = vi.hoisted(() => ({
  read: vi.fn<() => Promise<MachinePolicy>>(),
  write: vi.fn<(id: string, values: Record<string, unknown>) => Promise<MachinePolicy>>(),
  executors: vi.fn<() => Promise<MachineExecutor[]>>(),
}));
vi.mock('@/api/resources', () => ({
  machinePolicyQuery: () => ({ queryKey: ['machine-policy', 'machine'], queryFn: read }),
  machineQuery: () => ({ queryKey: ['machine', 'machine'] }), updateMachinePolicy: write,
}));
vi.mock('@/api/machineExecutors', () => ({
  machineExecutorsQuery: (id: string) => ({ queryKey: ['machine-executors', id], queryFn: executors }),
}));

function policy(value: string[] = ['plain', 'retired:tools']): MachinePolicy {
  return { machine: { id: 'machine', name: 'Laptop', online: true, placement: 'connected', platform_os: 'linux' }, reported_at: null, keys: [{
    key: 'exec.offers', value, bounds: 'environment', delegated: true, writable: true,
    definition: { key: 'exec.offers', title: 'Execution offers', meaning: 'Tools offered here.', sections: ['environment'], managed: [], cautious: [], delegable: true },
  }] };
}
function declared(): MachineExecutor[] {
  return [{ origin: 'builtin', state: null, executor: {
    kind: 'core', title: { en: 'Core tools', 'zh-CN': '基础工具' }, program: null, tool_sets: [], offers: [{
      value: 'core:plain', alias: 'plain', title: { en: 'Core tools', 'zh-CN': '基础工具' }, summary: { en: 'Tools from this release.', 'zh-CN': '本发行版提供的工具。' },
    }],
  } }, { origin: 'plugin', state: 'running', executor: {
    kind: 'third-runner', title: { en: 'Third program tools', 'zh-CN': '第三方程序工具' }, program: 'third-program', tool_sets: [], offers: [{
      value: 'third-runner:files', alias: null, title: { en: 'Third program tools', 'zh-CN': '第三方程序工具' }, summary: { en: 'Tools declared by this machine.', 'zh-CN': '由此机器声明的工具。' },
    }],
  } }];
}
async function openEditor() {
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: i18n.t('machines.policy.keys.exec_offers.title') }) }));
  return user;
}
beforeEach(() => { vi.clearAllMocks(); read.mockResolvedValue(policy()); executors.mockResolvedValue(declared()); write.mockResolvedValue(policy()); });

for (const locale of ['en', 'zh-CN'] as const) it(`uses declared offers and aliases, and allows a missing saved offer to be cleared (${locale})`, async () => {
  await setLocale(locale);
  try {
    show(<MachinePolicyTab machine={{ id: 'machine' }} />);
    await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: i18n.t('machines.policy.keys.exec_offers.title') }) });
    expect(executors).not.toHaveBeenCalled();
    const user = await openEditor();
    const core = await screen.findByRole('checkbox', { name: locale === 'en' ? 'Core tools' : '基础工具' });
    const third = screen.getByRole('checkbox', { name: locale === 'en' ? 'Third program tools' : '第三方程序工具' });
    const retained = screen.getByRole('checkbox', { name: i18n.t('machines.policy.savedOffer', { value: 'retired:tools' }) });
    expect(screen.getAllByRole('checkbox')).toHaveLength(3);
    expect(core).toBeChecked(); expect(third).not.toBeChecked(); expect(retained).toBeChecked();
    await user.click(third); await user.click(retained);
    expect(retained).not.toBeChecked(); expect(retained).toBeDisabled();
    await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
    await waitFor(() => expect(write).toHaveBeenCalledWith('machine', { 'exec.offers': ['core:plain', 'third-runner:files'] }));
    expect(read).toHaveBeenCalledTimes(1); expect(executors).toHaveBeenCalledTimes(1);
  } finally { await setLocale('en'); }
});

it('keeps an unreported saved offer when another declared offer is selected', async () => {
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  const user = await openEditor();
  await user.click(await screen.findByRole('checkbox', { name: 'Third program tools' }));
  await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
  await waitFor(() => expect(write).toHaveBeenCalledWith('machine', { 'exec.offers': ['core:plain', 'retired:tools', 'third-runner:files'] }));
});

it('names the loading wait and blocks saving until the offers arrive', async () => {
  executors.mockReturnValue(new Promise(() => undefined));
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  const user = await openEditor();
  expect(screen.getByText(i18n.t('machines.policy.loadingExecutors'))).toBeVisible();
  const save = screen.getByRole('button', { name: i18n.t('machines.policy.save') });
  expect(save).toHaveAttribute('aria-disabled', 'true');
  await user.click(save); expect(write).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
  expect(write).not.toHaveBeenCalled();
});

it('shows a failed executor read with retry, preserving the saved selection', async () => {
  executors.mockRejectedValueOnce(new NetworkError('failed'));
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  const user = await openEditor();
  expect(await screen.findByText(i18n.t('machines.policy.executorReadFailed'))).toBeVisible();
  const save = screen.getByRole('button', { name: i18n.t('machines.policy.save') });
  expect(save).toHaveAttribute('aria-disabled', 'true'); await user.click(save); expect(write).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByRole('checkbox', { name: 'Core tools' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: i18n.t('machines.policy.savedOffer', { value: 'retired:tools' }) })).toBeChecked();
  expect(executors).toHaveBeenCalledTimes(2);
});

it('accepts an empty report without inventing executor offers', async () => {
  read.mockResolvedValue(policy([])); executors.mockResolvedValue([]);
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  await openEditor();
  expect(await screen.findByText(i18n.t('machines.policy.noExecutors'))).toBeVisible();
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: i18n.t('machines.policy.save') })).not.toHaveAttribute('aria-disabled', 'true');
});

it('mounts one page of a hundred thousand reported offers and preserves an off-page selection', async () => {
  read.mockResolvedValue(policy(['plugin:set-99999']));
  executors.mockResolvedValue([{ origin: 'plugin', state: 'running', executor: {
    kind: 'plugin', title: { en: 'A large plugin' }, program: null, tool_sets: [], offers: Array.from({ length: 100_000 }, (_, index) => ({
      value: `plugin:set-${index}`, alias: null, title: { en: `Declared tools ${index}` }, summary: { en: 'A declared tool set.' },
    })),
  } }]);
  const view = show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  const user = await openEditor();
  await screen.findByRole('checkbox', { name: 'Declared tools 0' });
  for (let page = 1; page <= 3; page += 1) {
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
    expect(screen.getByRole('checkbox', { name: `Declared tools ${page * DETAIL_PAGE_ROWS}` })).toBeVisible();
    expect(view.container.querySelectorAll('[role="checkbox"]')).toHaveLength(DETAIL_PAGE_ROWS);
    expect(screen.queryByRole('checkbox', { name: 'Declared tools 0', hidden: true })).not.toBeInTheDocument();
  }
  await user.click(screen.getByRole('checkbox', { name: `Declared tools ${3 * DETAIL_PAGE_ROWS}` }));
  await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
  await waitFor(() => expect(write).toHaveBeenCalledWith('machine', { 'exec.offers': ['plugin:set-99999', `plugin:set-${3 * DETAIL_PAGE_ROWS}`] }));
  expect(executors).toHaveBeenCalledTimes(1);
});
