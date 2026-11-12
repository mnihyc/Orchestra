/** Capability actions retain the server's exact admission and name each native refusal. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import * as api from '@/api/ports';
import { PortsGrants } from '@/features/ports/Grants';
import { i18n, setLocale } from '@/i18n';
import { portCapability } from './ports-fixtures';
import { show } from './support';

afterEach(async () => { cleanup(); vi.restoreAllMocks(); await setLocale('en'); });
for (const language of ['en', 'zh-CN'] as const) {
  it(`keeps native-refused Grant and Take back visible with their reason in ${language}`, async () => {
    await setLocale(language); const t = i18n.getFixedT(language);
    const grant = vi.spyOn(api, 'grantPortCapability'); const revoke = vi.spyOn(api, 'revokePortCapability');
    const cases = [
      ['manage_required', 'manage'], ['not_offered', 'notOffered'], ['policy_denied', 'policy'],
      ['already_granted', 'granted'], ['approver_required', 'approver'], [null, 'unknown'],
    ] as const;
    const base = portCapability('place', 'ask');
    const view = show(<PortsGrants environmentId="environment-1" capabilities={[]} />);
    for (const [reason, key] of cases) {
      const value = { ...base, can_grant: false, can_revoke: false,
        ...(reason === null ? {} : { grant_unavailable_reason: reason }),
        ...{ revoke_unavailable_reason: 'no_grant' as const } };
      view.rerender(<PortsGrants environmentId="environment-1" capabilities={[value]} />);
      const row = screen.getByTestId('port-grant-place');
      const button = within(row).getByRole('button', { name: t('ports.grants.grant') });
      expect(button).toHaveAttribute('aria-disabled', 'true');
      expect(button).toHaveAttribute('aria-description', t(`ports.grants.reasons.${key}`));
      const take = within(row).getByRole('button', { name: t('ports.grants.revoke') });
      expect(take).toHaveAttribute('aria-disabled', 'true');
      expect(take).toHaveAttribute('aria-description', t('ports.grants.reasons.noGrant'));
      await userEvent.click(button); await userEvent.click(take);
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    }
    expect(grant).not.toHaveBeenCalled(); expect(revoke).not.toHaveBeenCalled();
  });
  it(`keeps the existing confirmation and closes its authority after a current refusal in ${language}`, async () => {
    await setLocale(language); const t = i18n.getFixedT(language);
    const base = portCapability('place', 'ask');
    const grant = vi.spyOn(api, 'grantPortCapability').mockResolvedValue(portCapability('place', 'ask', true));
    const view = show(<PortsGrants environmentId="environment-1" capabilities={[base]} />);
    await userEvent.click(screen.getByRole('button', { name: t('ports.grants.grant') }));
    const dialog = screen.getByRole('dialog');
    expect(grant).not.toHaveBeenCalled();
    view.rerender(<PortsGrants environmentId="environment-1" capabilities={[{ ...base, can_grant: false,
      ...{ grant_unavailable_reason: 'manage_required' as const } }]} />);
    const confirm = within(dialog).getByRole('button', { name: t('ports.grants.grant') });
    expect(confirm).toHaveAttribute('aria-disabled', 'true');
    expect(confirm).toHaveAttribute('aria-description', t('ports.grants.reasons.manage'));
    await userEvent.click(confirm); expect(grant).not.toHaveBeenCalled();
    view.rerender(<PortsGrants environmentId="environment-1" capabilities={[base]} />);
    const restored = within(screen.getByRole('dialog')).getByRole('button', { name: t('ports.grants.grant') });
    expect(restored).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(restored);
    expect(grant).toHaveBeenCalledWith('environment-1', 'port_place');
  });
  it(`keeps taking back a native grant possible when policy refuses new openings in ${language}`, async () => {
    await setLocale(language); const t = i18n.getFixedT(language);
    const value = { ...portCapability('place', 'deny', true),
      ...{ grant_unavailable_reason: 'policy_denied' as const } };
    const revoke = vi.spyOn(api, 'revokePortCapability').mockResolvedValue(undefined);
    show(<PortsGrants environmentId="environment-1" capabilities={[value]} />);
    expect(screen.getByRole('button', { name: t('ports.grants.grant') })).toHaveAttribute('aria-disabled', 'true');
    const take = screen.getByRole('button', { name: t('ports.grants.revoke') });
    expect(take).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(take);
    expect(revoke).not.toHaveBeenCalled();
    await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: t('ports.grants.revoke') }));
    expect(revoke).toHaveBeenCalledWith('environment-1', 'port_place');
  });
}
