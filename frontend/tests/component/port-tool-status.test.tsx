/** Native current port-tool status is an inspector line, never a second permission resolver. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { PortTools } from '@/features/inspector/PortTools';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); useAppearance.getState().set({ locale: 'en', theme: 'system' }); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`names inactive requested tools and preserves the independent Close tool in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.getState().set({ locale, theme }); const t = i18n.getFixedT(locale);
    const view = show(<PortTools value={{ requested: [], effective: [], inactive: [] }} />);
    expect(screen.getByText(t('ports.tools.none'))).toBeVisible();
    const reasons = [
      ['no_environment', 'noEnvironment', true], ['environment_unavailable', 'environment', true], ['no_machine', 'noMachine', true],
      ['unsupported', 'unsupported', true], ['owner_view_required', 'owner', true], ['policy_off', 'policy', true], ['local_off', 'local', true],
      ['program_unsupported', 'program', false], ['environment_tools_off', 'environmentTools', false], ['continuation_restricted', 'continuation', false],
    ] as const;
    for (const [reason, key, closingAvailable] of reasons) {
      view.rerender(<PortTools value={{ requested: ['list_ports', 'close_port'], effective: closingAvailable ? ['close_port'] : [],
        inactive: [{ tool: 'list_ports', reason }, ...(closingAvailable ? [] : [{ tool: 'close_port', reason }])], private_source: { name: 'Hidden audience', id: 'private-source' } }} />);
      const region = screen.getByRole('region', { name: t('ports.tools.title') });
      expect(region).toHaveTextContent(t(`ports.tools.reasons.${key}`));
      expect(region).not.toHaveTextContent('Hidden audience'); expect(region).not.toHaveTextContent('private-source');
      const closing = region.querySelector('[data-port-tool="close_port"]');
      expect(closing).toHaveTextContent(t(closingAvailable ? 'ports.tools.available' : 'ports.tools.inactive'));
    }
    view.rerender(<PortTools value={{ requested: ['list_ports', 'open_port', 'close_port'], effective: ['list_ports', 'close_port'],
      inactive: [{ tool: 'open_port', reason: 'no_audience' }] }} />);
    const region = screen.getByRole('region', { name: t('ports.tools.title') });
    expect(region).toHaveTextContent(t('ports.tools.reasons.audience'));
    expect(within(region).queryByRole('button')).not.toBeInTheDocument();
  });
}
it('keeps missing and failed reads distinct from refusals and never renders stale tool facts', async () => {
  const retry = vi.fn(); const t = i18n.getFixedT('en');
  const value = { requested: ['open_port'], effective: [], inactive: [{ tool: 'open_port', reason: 'no_audience' }] };
  const view = show(<PortTools value={undefined} />);
  expect(screen.getByText(t('ports.tools.unreported'))).toBeVisible();
  view.rerender(<PortTools value={value} error={new NetworkError('offline')} onRetry={retry} />);
  expect(screen.getByRole('alert')).toBeVisible();
  expect(screen.queryByText(t('ports.tools.reasons.audience'))).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: t('common.tryAgain') })); expect(retry).toHaveBeenCalledOnce();
  view.rerender(<PortTools value={{ requested: ['open_port'], effective: ['open_port'], inactive: [{ tool: 'open_port', reason: 'no_audience' }] }} />);
  expect(screen.getByText(t('ports.tools.unreported'))).toBeVisible();
});
