/** Only a confirmed carrier is displayed as attached; failures retain plain recovery copy. */
import { screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { TunnelGateway } from '@/features/gateways/TunnelGateway';
import { i18n } from '@/i18n';
import { apiErrorSentence } from '@/i18n/errors';
import { show } from '../support';
import { catalogue, look, modes, restore, tunnel } from './fixtures';

afterEach(restore);
it.each(modes)('shows the attached gateway separately from the source primary in $locale/$theme at $width', async (mode) => {
  await look(mode);
  const view = show(<TunnelGateway status={tunnel} gateways={catalogue.gateways} />);
  expect(screen.getByText('Asia tools', { exact: true })).toBeVisible();
  expect(view.container).not.toHaveTextContent('Europe gateway');
  expect(view.container).not.toHaveTextContent('source_primary_gateway_id');
});

it('never labels a preferred candidate as the attached carrier while pending or unknown', () => {
  const view = show(<TunnelGateway status={{ ...tunnel, state: 'pending', attached_gateway_id: null }} gateways={catalogue.gateways} />);
  expect(view.container).toHaveTextContent(i18n.t('regionalGateways.tunnelState.pending'));
  expect(view.container).not.toHaveTextContent('Asia tools');
  view.rerender(<TunnelGateway status={{ ...tunnel, state: 'unknown', attached_gateway_id: null }} gateways={catalogue.gateways} />);
  expect(view.container).toHaveTextContent(i18n.t('regionalGateways.notReported'));
  expect(view.container).not.toHaveTextContent('Asia tools');
});

it.each(['en', 'zh-CN'] as const)('shows the safe unreachable reason and attempted labels without an automatic retry (%s)', async (locale) => {
  await look({ locale, theme: 'light', width: 400 });
  const detail = 'Europe gateway: source could not attach; Asia tools: peer could not attach.';
  const view = show(<TunnelGateway status={{ ...tunnel, state: 'unavailable', attached_gateway_id: null,
    refusal: { code: 'tunnel.gateway_unreachable', detail, attempted_gateways: ['eu', 'asia', 'eu'] } }} gateways={catalogue.gateways} />);
  expect(view.container).toHaveTextContent(i18n.t('regionalGateways.unreachable'));
  expect(apiErrorSentence('tunnel.gateway_unreachable', 503, i18n.t, locale)).toBe(i18n.t('regionalGateways.unreachable'));
  expect(view.container).toHaveTextContent(detail);
  const attempted = screen.getByRole('list', { name: i18n.t('regionalGateways.attempted') });
  expect(attempted).toHaveTextContent('Asia tools');
  expect(within(attempted).getAllByRole('listitem')).toHaveLength(2);
  expect(view.container).not.toHaveTextContent('tunnel.gateway_unreachable');
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('names a local pair without inventing a tunnel gateway', () => {
  const view = show(<TunnelGateway status={{ ...tunnel, state: 'local', attached_gateway_id: null, plan: null }} gateways={catalogue.gateways} />);
  expect(view.container).toHaveTextContent(i18n.t('regionalGateways.tunnelState.local'));
  expect(view.container).not.toHaveTextContent('Asia tools');
});

it('retains a confirmed carrier identity when its catalogue label is missing', () => {
  const view = show(<TunnelGateway status={{ ...tunnel, attached_gateway_id: 'unlisted', plan: null }} gateways={[]} />);
  expect(screen.getByText('unlisted', { exact: true })).toBeVisible();
  expect(view.container).not.toHaveTextContent(i18n.t('regionalGateways.unavailable', { id: 'unlisted' }));
  expect(view.container).not.toHaveTextContent(i18n.t('regionalGateways.unreachable'));
});
