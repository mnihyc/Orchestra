/** A host's page offers to stop or delete a provider's runtime by force when the refusal says that forcing changes it, and only then; each in both languages. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { removeHostRuntime, stopHostRuntime } from '@/api/resources';
import { HostRuntimes } from '@/features/resources/hosts/Runtimes';
import type { Runtime } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailEnvironment, detailHost, detailTemplate, detailUnitProvider } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ runtimes: vi.fn<() => Promise<Runtime[]>>() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(), removeHostRuntime: vi.fn(), stopHostRuntime: vi.fn(), hostRuntimesQuery: () => ({ queryKey: ['host-runtime-force'], queryFn: reads.runtimes, staleTime: Infinity }) }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), Link: ({ to, params, children }: { to: string; params: { id: string }; children: ReactNode }) => <a href={to.replace('$id', params.id)}>{children}</a> }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

const runtime: Runtime = { id: 'runtime', host_id: detailHost.id, kind: 'provider', driver: 'docker', generation: 2, environment: null, provider: detailUnitProvider, machine: null,
  state: 'running', state_detail: null, unit_name: 'orch-provider-private-unit', successor_of: null, template: detailTemplate, remove_at: null, last_active_at: null, network: detailEnvironment.network, updates: 0 };
const name = detailUnitProvider.name;
/** What the API answers when an act on a provider's runtime is held back by a reason it names. */
function refusal(reason: string, extra: Record<string, unknown>): ApiError {
  return new ApiError({ type: 'orch:error:provider.unavailable', code: 'provider.unavailable', title: 'Not ready', detail: 'Its runtime cannot be started or stopped now: an English sentence.', status: 409, request_id: 'request', reason, ...extra }, null);
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`offers the stop by force after a refusal that forcing changes, and says what is in the way (${locale})`, async () => {
    await setLocale(locale);
    reads.runtimes.mockResolvedValue([runtime]);
    vi.mocked(stopHostRuntime).mockRejectedValueOnce(refusal('host_silent', { can_force: true })).mockResolvedValueOnce(runtime);
    const user = userEvent.setup();
    show(<HostRuntimes host={detailHost} />);
    await user.click(await screen.findByRole('button', { name: i18n.t('hosts.runtimes.menu', { name }) }));
    await user.click(screen.getByRole('menuitem', { name: i18n.t('hosts.runtimes.stop') }));
    expect(await screen.findByText(i18n.t('errors.reason.host_silent'))).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('hosts.runtimes.stopForce', { name }) }));
    await waitFor(() => expect(stopHostRuntime).toHaveBeenCalledTimes(2));
    expect(stopHostRuntime).toHaveBeenNthCalledWith(1, detailHost.id, 'runtime', false, false);
    expect(stopHostRuntime).toHaveBeenNthCalledWith(2, detailHost.id, 'runtime', false, true);
  });

  it(`offers no stop by force after a refusal that forcing does not change (${locale})`, async () => {
    await setLocale(locale);
    reads.runtimes.mockResolvedValue([runtime]);
    vi.mocked(stopHostRuntime).mockRejectedValueOnce(refusal('other_work', { can_force: false }));
    const user = userEvent.setup();
    show(<HostRuntimes host={detailHost} />);
    await user.click(await screen.findByRole('button', { name: i18n.t('hosts.runtimes.menu', { name }) }));
    await user.click(screen.getByRole('menuitem', { name: i18n.t('hosts.runtimes.stop') }));
    expect(await screen.findByText(i18n.t('errors.reason.other_work'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('hosts.runtimes.stopForce', { name }) })).not.toBeInTheDocument();
  });

  it(`offers the removal by force after a refusal that forcing changes, and asks for it in the same dialog (${locale})`, async () => {
    await setLocale(locale);
    reads.runtimes.mockResolvedValue([runtime]);
    vi.mocked(removeHostRuntime).mockRejectedValueOnce(refusal('own_work', { work: 'update', step: 'replace', can_force: true })).mockResolvedValueOnce(undefined);
    const user = userEvent.setup();
    show(<HostRuntimes host={detailHost} />);
    await user.click(await screen.findByRole('button', { name: i18n.t('hosts.runtimes.menu', { name }) }));
    await user.click(screen.getByRole('menuitem', { name: i18n.t('hosts.runtimes.remove') }));
    expect(screen.queryByRole('checkbox', { name: i18n.t('hosts.runtimes.force') })).not.toBeInTheDocument();
    await user.click(screen.getByRole('textbox', { name: i18n.t('hosts.runtimes.type', { name }) }));
    await user.paste(name);
    await user.click(screen.getByRole('button', { name: i18n.t('hosts.runtimes.removeConfirm', { name }) }));
    expect(await screen.findByText(i18n.t('errors.reason.own_work', { work: i18n.t('errors.reasonWork.update'), step: i18n.t('providers.runtime.phase.replace') }))).toBeVisible();
    await user.click(screen.getByRole('checkbox', { name: i18n.t('hosts.runtimes.force') }));
    await user.click(screen.getByRole('button', { name: i18n.t('hosts.runtimes.removeConfirm', { name }) }));
    await waitFor(() => expect(removeHostRuntime).toHaveBeenCalledTimes(2));
    expect(removeHostRuntime).toHaveBeenNthCalledWith(1, detailHost.id, 'runtime', runtime.unit_name, false, false);
    expect(removeHostRuntime).toHaveBeenNthCalledWith(2, detailHost.id, 'runtime', runtime.unit_name, false, true);
  });
}
