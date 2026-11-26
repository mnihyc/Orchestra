/** Primary selection and connector measurements never become a browser-selected route. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { MachineGatewayCard } from '@/features/gateways/MachineGateway';
import { i18n } from '@/i18n';
import { show } from '../support';
import { catalogue, connector, look, modes, restore } from './fixtures';

afterEach(restore);
it('finds a gateway beyond the first measurements page without expanding the whole catalogue',async()=>{
  const gateways=Array.from({length:1000},(_,index)=>({...catalogue.gateways[0]!,id:`gateway-${index}`,label:`Gateway ${index}`,region:'Region'}));
  const user=userEvent.setup();
  show(<MachineGatewayCard status={connector} gateways={gateways}/>);
  expect(screen.queryByText('Gateway 999',{exact:true})).not.toBeInTheDocument();
  await user.type(screen.getByRole('searchbox',{name:i18n.t('regionalGateways.findGateway')}),'Gateway 999');
  expect(await screen.findByText('Gateway 999',{exact:true})).toBeVisible();
});
it.each(modes)('shows reported Automatic latencies in $locale/$theme at $width', async (mode) => {
  await look(mode);
  show(<MachineGatewayCard status={connector} gateways={catalogue.gateways} />);
  const samples = screen.getByRole('list', { name: i18n.t('regionalGateways.measurements') });
  expect(within(samples).getByText('Europe gateway', { exact: true })).toBeVisible();
  expect(within(samples).getByText(i18n.t('regionalGateways.milliseconds', { value: '190' }))).toBeVisible();
  expect(within(samples).getByText(i18n.t('regionalGateways.milliseconds', { value: '18' }))).toBeVisible();
  expect(screen.getByText(i18n.t('regionalGateways.automatic'), { exact: true })).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('regionalGateways.primary') }) }))
    .toHaveAttribute('data-help-heading', 'choose-a-gateway');
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /save|保存/i })).not.toBeInTheDocument();
});

it('retains an unavailable explicit choice while offline and hides the Automatic measurements', () => {
  show(<MachineGatewayCard status={{ ...connector, selection: 'selected', selected_gateway_id: 'removed', primary_gateway_id: null, connected: false }} gateways={catalogue.gateways} />);
  expect(screen.getByText(i18n.t('regionalGateways.unavailable', { id: 'removed' }))).toBeVisible();
  expect(screen.getByText(i18n.t('regionalGateways.selected'), { exact: true })).toBeVisible();
  expect(screen.getByText(i18n.t('regionalGateways.offline'), { exact: true })).toBeVisible();
  expect(screen.queryByRole('list', { name: i18n.t('regionalGateways.measurements') })).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('regionalGateways.automatic'), { exact: true })).not.toBeInTheDocument();
});

it('does not turn a missing report into zero latency or a guessed primary', () => {
  const view = show(<MachineGatewayCard status={{ ...connector, selection: null, primary_gateway_id: null, measured_at: null, measurements: [] }} gateways={catalogue.gateways} />);
  expect(view.container).toHaveTextContent(i18n.t('regionalGateways.notReported'));
  expect(view.container).not.toHaveTextContent(i18n.t('regionalGateways.milliseconds', { value: '0' }));
});

it('retains a connected primary identity when its selected advertisement has been removed', () => {
  show(<MachineGatewayCard status={{ ...connector, selection: 'selected', selected_gateway_id: 'removed', primary_gateway_id: 'removed' }} gateways={catalogue.gateways} />);
  expect(screen.getByText('removed', { exact: true })).toBeVisible();
  expect(screen.getByText(i18n.t('regionalGateways.unavailable', { id: 'removed' }))).toBeVisible();
  expect(screen.getByText(i18n.t('regionalGateways.connected'), { exact: true })).toBeVisible();
});
