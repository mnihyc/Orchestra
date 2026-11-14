/** A provider's runtime says the step its work has come to and why it stands where it does, and a stop that was refused for a reason that forcing changes is offered by force; each in both languages. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { stopRuntime } from '@/api/provider-runtime';
import { ProviderRuntime } from '@/features/resources/providers/Runtime';
import type { Provider } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { detailUnitProvider } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), Link: ({ children }: { children: ReactNode }) => <a href="#link">{children}</a> }));
vi.mock('@/features/resources/machines/Policy', () => ({ MachineLimits: () => null }));
vi.mock('@/features/resources/providers/RuntimeConfiguration', () => ({ RuntimeConfiguration: () => null }));
vi.mock('@/api/provider-runtime', () => ({ startRuntime: vi.fn(), stopRuntime: vi.fn() }));
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

function runtimeOfFixture(): NonNullable<Provider['runtime']> {
  const runtime = detailUnitProvider.runtime;
  if (runtime === null) throw new Error('the fixture provider has a runtime');
  return runtime;
}
const unit = runtimeOfFixture();
type Work = NonNullable<Provider['runtime']>['job'];
/** A provider whose runtime is `state`, with the work on it and what is said of why it stands there. */
function provider(state: string, job: Work = null, detail: string | null = null): Provider {
  return { ...detailUnitProvider, runtime: { ...unit, state, job, detail } };
}
const work: NonNullable<Work> = { id: 'work', kind: 'provider_provision', state: 'running', phase: 'enroll', percent: 62 };
/** What the API answers when a stop is held back by a reason it names. */
function refusal(reason: string, extra: Record<string, unknown>): ApiError {
  return new ApiError({ type: 'orch:error:provider.unavailable', code: 'provider.unavailable', title: 'Not ready', detail: 'Its runtime cannot be started or stopped now: an English sentence.', status: 409, request_id: 'request', reason, ...extra }, null);
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the step the work on a runtime has come to (${locale})`, async () => {
    await setLocale(locale);
    show(<ProviderRuntime provider={provider('creating', work)} />);
    expect(screen.getByText(i18n.t('providers.runtime.step', { step: i18n.t('providers.runtime.phase.enroll'), percent: 62 }))).toBeVisible();
  });

  it(`names the steps of taking a runtime away by their own words (${locale})`, async () => {
    await setLocale(locale);
    show(<ProviderRuntime provider={provider('retired', { ...work, kind: 'environment_remove', phase: 'volumes' })} />);
    expect(screen.getByText(i18n.t('providers.runtime.step', { step: i18n.t('providers.runtime.removalPhase.volumes'), percent: 62 }))).toBeVisible();
    expect(i18n.t('providers.runtime.removalPhase.volumes')).not.toBe(i18n.t('providers.runtime.phase.volumes'));
  });

  it(`says why a runtime stands where it does: in words for a reason the system names, as it is for the host's own (${locale})`, async () => {
    await setLocale(locale);
    show(<ProviderRuntime provider={provider('failed', null, 'provider_unit_unfinished')} />);
    expect(screen.getByText(i18n.t('providers.runtime.reason'))).toBeVisible();
    expect(screen.getByText(i18n.t('providerRecovery.runtimeUnfinished'))).toBeVisible();
    cleanup();
    show(<ProviderRuntime provider={provider('failed', null, 'docker: no space left on device')} />);
    expect(screen.getByText('docker: no space left on device')).toBeVisible();
  });

  it(`offers the stop by force when the refusal says that forcing changes it, and says what is in the way (${locale})`, async () => {
    await setLocale(locale);
    vi.mocked(stopRuntime).mockRejectedValueOnce(refusal('own_work', { work: 'make', step: 'enroll', can_force: true })).mockResolvedValueOnce(planProvider());
    const user = userEvent.setup();
    show(<ProviderRuntime provider={provider('creating', work)} />);
    await user.click(screen.getByRole('button', { name: i18n.t('providerRuntime.stop') }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('providerRuntime.stop') }));
    expect(await within(dialog).findByText(i18n.t('errors.reason.own_work', { work: i18n.t('errors.reasonWork.make'), step: i18n.t('providers.runtime.phase.enroll') }))).toBeVisible();
    expect(within(dialog).getByText(i18n.t('providerRuntime.forceHelp'))).toBeVisible();
    await user.click(within(dialog).getByRole('button', { name: i18n.t('providerRuntime.stopForce') }));
    await waitFor(() => expect(stopRuntime).toHaveBeenCalledTimes(2));
    expect(stopRuntime).toHaveBeenNthCalledWith(1, 'unit-provider', true, false);
    expect(stopRuntime).toHaveBeenNthCalledWith(2, 'unit-provider', true, true);
  });

  it(`offers no force for a refusal that forcing does not change (${locale})`, async () => {
    await setLocale(locale);
    vi.mocked(stopRuntime).mockRejectedValueOnce(refusal('other_work', { can_force: false }));
    const user = userEvent.setup();
    show(<ProviderRuntime provider={provider('running')} />);
    await user.click(screen.getByRole('button', { name: i18n.t('providerRuntime.stop') }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('providerRuntime.stop') }));
    expect(await within(dialog).findByText(i18n.t('errors.reason.other_work'))).toBeVisible();
    expect(within(dialog).queryByRole('button', { name: i18n.t('providerRuntime.stopForce') })).not.toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: i18n.t('providerRuntime.stop') })).toBeVisible();
  });
}
