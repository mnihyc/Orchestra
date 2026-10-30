/** Historical machine reports do not multiply the provider's named profile rows or restart another profile. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { restartHarness } from '@/api/resources';
import { HarnessProfiles, ProviderRuntime } from '@/features/resources/providers/Runtime';
import type { HarnessState } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { planProvider } from '../unit/provider-plan-fixtures';
import { detailAgent, detailMachineRef } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@/api/resources', async (load) => ({ ...await load<object>(), restartHarness: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), Link: ({ to, params, children }: { to: string; params: { id: string }; children: ReactNode }) => <a href={to.replace('$id', params.id)}>{children}</a> }));
const appearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), previousTheme = document.documentElement.dataset.theme;
beforeEach(() => { vi.mocked(restartHarness).mockReset(); vi.mocked(restartHarness).mockResolvedValue(planProvider()); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', wasDark); if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme; await setLocale('en'); });
function report(machine: string, sessions: number): HarnessState {
  return { agent: detailAgent, machine: { ...detailMachineRef, id: machine, name: `Host ${machine}` }, status: 'online', status_detail: null, sessions_open: sessions, harness_version: '1.2.3', last_active_at: null };
}

for (const locale of ['en', 'zh-CN'] as const) it(`retains the program's last error visibly after its supervised restart (${locale})`, async () => {
  await setLocale(locale);
  const current = { ...report('a', 0), last_error: { at: '2026-10-20T12:00:00Z', detail: 'app-server exited with code 7: final diagnostic' } };
  const old = { ...report('b', 0), last_error: { at: '2026-10-19T12:00:00Z', detail: 'older failure' } };
  show(<ProviderRuntime configuration={false} provider={planProvider({ machine: current.machine, harnesses: [current, old] })} />);
  expect(screen.getByText(i18n.t('providers.runtime.harnessStatus.online'), { exact: true })).toBeVisible();
  expect(screen.getByText(i18n.t('providers.runtime.lastProgramError'))).toBeVisible();
  expect(screen.getByText(current.last_error.detail)).toBeVisible();
  expect(screen.queryByText('older failure')).not.toBeInTheDocument();
  expect(restartHarness).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps one named row per profile, exact machine evidence and a profile-only restart (${locale}/${theme})`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const user = userEvent.setup(), first = report('a', 1), second = report('b', 2);
  const other = { ...report('c', 5), agent: { ...detailAgent, id: 'other-profile', name: 'A second Codex profile' } };
  const provider = planProvider({ machine: first.machine, harnesses: [first, second, { ...second }, other] });
  vi.mocked(restartHarness).mockResolvedValue(provider);
  const view = show(<HarnessProfiles provider={provider} />);
  const list = screen.getByRole('list', { name: i18n.t('providers.runtime.harnesses') });
  expect(within(list).getAllByRole('listitem')).toHaveLength(2);
  const name = screen.getByRole('link', { name: detailAgent.name }); expect(name).toHaveAttribute('href', '/resources/agents/agent');
  expect(screen.getAllByRole('link', { name: detailAgent.name })).toHaveLength(1); expect(screen.getByRole('link', { name: other.agent.name })).toBeVisible();
  const row = name.closest('li'); if (row === null) throw new Error('The profile needs a row');
  expect(within(row).getByText(i18n.t('providers.runtime.sessions', { count: 3 }), { exact: true })).toBeVisible();
  expect(screen.queryByText('Host b')).not.toBeInTheDocument(); expect(restartHarness).not.toHaveBeenCalled();
  await user.click(within(row).getByText(i18n.t('common.details'), { exact: true }));
  expect(await screen.findByText('Host b')).toBeVisible();
  await user.click(within(row).getByRole('button', { name: i18n.t('providers.runtime.restart') }));
  await waitFor(() => expect(restartHarness).toHaveBeenCalledExactlyOnceWith(provider.id, detailAgent.id));
  view.rerender(<HarnessProfiles provider={{ ...provider, harnesses: [...provider.harnesses, { ...second }, { ...second }] }} />);
  expect(within(list).getAllByRole('listitem')).toHaveLength(2);
  expect(within(row).getByText(i18n.t('providers.runtime.sessions', { count: 3 }), { exact: true })).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) it(`does not call an old machine report the current profile's state (${locale})`, async () => {
  await setLocale(locale);
  const old = report('old', 0), provider = planProvider({ machine: { ...old.machine, id: 'new' }, harnesses: [old] });
  show(<HarnessProfiles provider={provider} />);
  expect(screen.getByRole('link', { name: detailAgent.name })).toBeVisible();
  expect(screen.getByText(i18n.t('resourceDetails.harnessUnreported'))).toBeVisible();
  expect(screen.queryByText(i18n.t('providers.runtime.harnessStatus.online'), { exact: true })).not.toBeInTheDocument();
  expect(restartHarness).not.toHaveBeenCalled();
  await userEvent.click(screen.getByText(i18n.t('common.details'), { exact: true }));
  expect(await screen.findByText('Host old', { exact: true })).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) it(`names the old machine when its error is newer than the current machine report (${locale})`, async () => {
  await setLocale(locale);
  const current = report('current', 0);
  const previous = { ...report('previous', 0), last_error: { at: '2026-10-20T12:00:00Z', detail: 'old machine exited unexpectedly' } };
  show(<HarnessProfiles provider={planProvider({ machine: current.machine, harnesses: [current, previous] })} />);
  expect(screen.getByText(previous.last_error.detail)).toBeVisible();
  expect(screen.getByRole('link', { name: previous.machine.name })).toBeVisible();
  expect(screen.getByRole('link', { name: previous.machine.name })).toHaveAttribute('href', '/resources/machines/previous');
});
