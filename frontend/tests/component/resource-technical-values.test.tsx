/** Ordinary resource facts show names; exact paths, fingerprints and unit identities remain deliberate actions. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { removeHostRuntime } from '@/api/resources';
import type * as ToastApi from '@/ui/Toast';
import { HostRuntimes } from '@/features/resources/hosts/Runtimes';
import { CommandValue, PathValue, RuntimeChange, runtimeChangeText } from '@/features/resources/TechnicalValue';
import { PlanLists } from '@/features/change/Lists';
import type { Runtime } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailEnvironment, detailHost, detailTemplate, detailUnitProvider } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ runtimes: vi.fn<() => Promise<Runtime[]>>() }));
const notices = vi.hoisted(() => ({ dismiss: new Set<() => void>() }));
vi.mock('@/ui/Toast', async (load) => {
  const actual = await load<typeof ToastApi>();
  return { ...actual, toast: (options: ToastApi.ToastOptions) => {
    const dismiss = actual.toast(options); notices.dismiss.add(dismiss); return dismiss;
  } };
});
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(), removeHostRuntime: vi.fn(), hostRuntimesQuery: () => ({ queryKey: ['technical-host-runtimes'], queryFn: reads.runtimes, staleTime: Infinity }) }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), Link: ({ to, params, children }: { to: string; params: { id: string }; children: ReactNode }) => <a href={to.replace('$id', params.id)}>{children}</a> }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
const appearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), previousTheme = document.documentElement.dataset.theme;
beforeEach(() => { vi.clearAllMocks(); vi.mocked(removeHostRuntime).mockResolvedValue(undefined); });
afterEach(async () => {
  // The application keeps notices across page mounts. Each case owns and dismisses its real notices.
  act(() => { for (const dismiss of notices.dismiss) dismiss(); notices.dismiss.clear(); });
  cleanup(); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', wasDark); if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme; await setLocale('en');
});
const path = '/home/private-account/.orch/environments/research/home', unit = 'orch-environment-01a14184-private-unit';
const runtime: Runtime = { id: 'runtime', host_id: detailHost.id, kind: 'environment', driver: 'docker', generation: 2, environment: detailEnvironment, provider: null, machine: null,
  state: 'stopped', state_detail: null, unit_name: unit, successor_of: null, template: detailTemplate, remove_at: null, last_active_at: null, network: detailEnvironment.network, updates: 0 };

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`reveals an exact home path on focus and copies it without printing it ordinarily (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(); show(<PathValue path={path} />);
    expect(screen.getByText('…/research/home')).toBeVisible(); expect(screen.queryByText(path, { exact: true })).not.toBeInTheDocument();
    await user.tab(); expect(await screen.findByRole('tooltip')).toHaveTextContent(path);
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.copyPath') }));
    expect(await navigator.clipboard.readText()).toBe(path);
  });
  it(`keeps full runtime fingerprints in Details and retains a genuine update (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), from = 'a'.repeat(64), to = 'b'.repeat(64);
    show(<RuntimeChange change={{ what: 'settings', name: 'Runtime definition', from, to }} />);
    expect(screen.getByText(i18n.t('resourceDetails.definitionChanged'))).toBeVisible(); expect(screen.queryByText(from)).not.toBeInTheDocument(); expect(screen.queryByText(to)).not.toBeInTheDocument();
    await user.click(screen.getByText(i18n.t('common.details'), { exact: true }));
    expect(await screen.findByText(from, { exact: true })).toBeVisible(); expect(screen.getByText(to, { exact: true })).toBeVisible();
  });
  it(`keeps the update plan's complete fingerprints available without making them its sentence (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), technical = { what: 'settings' as const, name: 'Runtime definition', from: 'c'.repeat(64), to: 'd'.repeat(64) };
    show(<PlanLists changes={[{ id: 'runtime', tone: 'plain', text: runtimeChangeText(technical, i18n.getFixedT(locale)), technical }]} stays={[]} ends={[]} />);
    expect(screen.getByText(i18n.t('resourceDetails.definitionChanged'))).toBeVisible(); expect(screen.queryByText(technical.from)).not.toBeInTheDocument();
    await user.click(screen.getByText(i18n.t('common.details'), { exact: true })); expect(await screen.findByText(technical.from, { exact: true })).toBeVisible(); expect(screen.getByText(technical.to, { exact: true })).toBeVisible();
  });
  it(`names a host runtime and guards deletion by its resource name while submitting the exact unit (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    reads.runtimes.mockResolvedValue([runtime]); const user = userEvent.setup(); show(<HostRuntimes host={detailHost} />);
    expect(await screen.findByRole('link', { name: detailEnvironment.name })).toBeVisible(); expect(screen.queryByText(unit, { exact: true })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.copyId') })); expect(await navigator.clipboard.readText()).toBe(unit); expect(removeHostRuntime).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('hosts.runtimes.menu', { name: detailEnvironment.name }) })); await user.click(screen.getByRole('menuitem', { name: i18n.t('hosts.runtimes.remove') }));
    const confirm = screen.getByRole('button', { name: i18n.t('hosts.runtimes.removeConfirm', { name: detailEnvironment.name }) }); expect(confirm).toHaveAttribute('aria-disabled', 'true');
    await user.click(confirm); expect(removeHostRuntime).not.toHaveBeenCalled();
    await user.click(screen.getByRole('textbox', { name: i18n.t('hosts.runtimes.type', { name: detailEnvironment.name }) })); await user.paste(detailEnvironment.name);
    await user.click(confirm); await waitFor(() => expect(removeHostRuntime).toHaveBeenCalledExactlyOnceWith(detailHost.id, runtime.id, unit, false));
  });
  for (const volumes of [false, true]) it(`keeps provider-unit deletion offered and confirms its exact unit with the volume choice (${locale}/${theme}/${volumes})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const providerRuntime: Runtime = { ...runtime, kind: 'provider', environment: null, provider: detailUnitProvider, unit_name: 'orch-provider-private-unit', state: 'running' };
    reads.runtimes.mockResolvedValue([providerRuntime]); const user = userEvent.setup(); show(<HostRuntimes host={detailHost} />);
    expect(await screen.findByRole('link', { name: detailUnitProvider.name })).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('hosts.runtimes.menu', { name: detailUnitProvider.name }) }));
    await user.click(screen.getByRole('menuitem', { name: i18n.t('hosts.runtimes.remove') }));
    expect(screen.getByText(i18n.t('hosts.runtimes.removeProviderBody', { host: detailHost.name }))).toBeVisible();
    const confirm = screen.getByRole('button', { name: i18n.t('hosts.runtimes.removeConfirm', { name: detailUnitProvider.name }) });
    expect(confirm).toHaveAttribute('aria-disabled', 'true'); await user.click(confirm); expect(removeHostRuntime).not.toHaveBeenCalled();
    const field = screen.getByRole('textbox', { name: i18n.t('hosts.runtimes.type', { name: detailUnitProvider.name }) });
    await user.click(field); await user.paste(providerRuntime.unit_name); expect(confirm).toHaveAttribute('aria-disabled', 'true');
    await user.clear(field); await user.paste(detailUnitProvider.name);
    const checkbox = screen.getByRole('checkbox', { name: i18n.t('hosts.runtimes.volumes') }); expect(checkbox).not.toBeChecked();
    if (volumes) await user.click(checkbox);
    await user.click(confirm);
    await waitFor(() => expect(removeHostRuntime).toHaveBeenCalledExactlyOnceWith(detailHost.id, providerRuntime.id, providerRuntime.unit_name, volumes));
    expect(await screen.findByText(i18n.t('hosts.runtimes.removed', { name: detailUnitProvider.name }))).toBeVisible();
  });
}

it('copies an exact registered command while keeping its home path and arguments on demand', async () => {
  const user = userEvent.setup(); show(<CommandValue command={['/home/private-account/bin/agent', '--root', '/home/private-account/project with spaces']} />);
  expect(screen.getByText('…/bin/agent')).toBeVisible(); expect(screen.queryByText(/private-account/)).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.copyCommand') }));
  expect(await navigator.clipboard.readText()).toBe('/home/private-account/bin/agent --root "/home/private-account/project with spaces"');
});
