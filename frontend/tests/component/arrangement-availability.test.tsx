/** Provider switching keeps every native arrangement and explains the actual closed door. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ProviderArrangementChoice } from '@/features/start/Arrangement';
import type { ArrangementConsent, ArrangementType } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
afterEach(async () => { cleanup(); await setLocale('en'); });

function consent(type: ArrangementType, allowed: boolean, available: boolean): ArrangementConsent {
  return { type, allowed, available, puts: '', form: null };
}

async function openReasons(user: ReturnType<typeof userEvent.setup>) {
  const term = i18n.t('start.arrangement.label');
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term }) }));
  return within(screen.getByRole('dialog', { name: term }));
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps refused switch choices named and disabled without replacing a retained choice (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const change = vi.fn();
    const provider = planProvider({ arrangements: [
      consent('separated', true, true), consent('key_separated', false, true),
      consent('credential_shared', true, false), consent('handed_over', true, true),
    ] });
    show(<ProviderArrangementChoice provider={provider} value="credential_shared" onChange={change} />);
    const group = screen.getByRole('radiogroup', { name: i18n.t('start.arrangement.label') });
    const ownerRefused = within(group).getByRole('radio', { name: i18n.t('choiceCatalog.arrangement.key_separated.name') });
    const policyRefused = within(group).getByRole('radio', { name: i18n.t('choiceCatalog.arrangement.credential_shared.name') });
    expect(within(group).getAllByRole('radio')).toHaveLength(5);
    expect(ownerRefused).toBeDisabled();
    expect(ownerRefused).toHaveAccessibleDescription(expect.stringContaining(i18n.t('arrangementAvailability.owner')));
    expect(policyRefused).toBeDisabled();
    expect(policyRefused).toBeChecked();
    expect(policyRefused).toHaveAccessibleDescription(expect.stringContaining(i18n.t('arrangementAvailability.machine')));
    expect(group.querySelector('[data-choice-description]')).toBeNull();
    expect(policyRefused.querySelector('svg')).not.toBeNull();
    await user.click(ownerRefused);
    await user.click(policyRefused);
    expect(change).not.toHaveBeenCalled();
    await user.click(within(group).getByRole('radio', { name: i18n.t('choiceCatalog.arrangement.handed_over.name') }));
    expect(change).toHaveBeenCalledExactlyOnceWith('handed_over');
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    const reasons = await openReasons(user);
    expect(reasons.getByText(i18n.t('arrangementAvailability.owner'))).toBeVisible();
    expect(reasons.getByText(i18n.t('arrangementAvailability.machine'))).toBeVisible();
  });

  it(`distinguishes no usable setup from no reported setup and keeps Automatic inert (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const change = vi.fn();
    const provider = planProvider({ arrangements: [consent('separated', false, true), consent('key_separated', true, false)] });
    const view = show(<ProviderArrangementChoice provider={provider} value={null} onChange={change} />);
    const group = screen.getByRole('radiogroup', { name: i18n.t('start.arrangement.label') });
    expect(within(group).getAllByRole('radio')).toHaveLength(3);
    for (const radio of within(group).getAllByRole('radio')) expect(radio).toBeDisabled();
    const reasons = await openReasons(user);
    expect(reasons.getByText(i18n.t('arrangementAvailability.none'))).toBeVisible();
    view.rerender(<ProviderArrangementChoice provider={{ ...provider, arrangements: [] }} value={null} onChange={change} />);
    expect(within(group).getAllByRole('radio')).toHaveLength(1);
    expect(reasons.getByText(i18n.t('arrangementAvailability.unreported'))).toBeVisible();
    expect(reasons.queryByText(i18n.t('arrangementAvailability.none'))).not.toBeInTheDocument();
    expect(change).not.toHaveBeenCalled();
  });
}
