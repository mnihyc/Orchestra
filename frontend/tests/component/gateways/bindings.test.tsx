/** Native reads keep machine and thread identities distinct; catalogue receipts repair one read. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as GatewayApi from '@/api/gateways';
import { applyChanges, findAct } from '@/api/acts';
import { Gateways } from '@/features/admin/Gateways';
import { MachineGatewayStatus } from '@/features/gateways/MachineGatewayStatus';
import { applyRow } from '@/api/cache';
import { ThreadGatewayDetails, ThreadGatewayStatus } from '@/features/gateways/ThreadGatewayStatus';
import type { ConnectorGatewayStatus, GatewayCatalogue, GatewayCatalogueUpdateBody, GatewayCatalogueUpdated, ThreadTunnelGatewayStatus, MachineGatewaySetBody, MachineGatewaySetReceipt } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from '../support';
import { catalogue, connector, look, restore, tunnel } from './fixtures';

const calls = vi.hoisted(() => ({ catalogue: vi.fn<() => Promise<GatewayCatalogue>>(),
  machine: vi.fn<(id: string) => Promise<ConnectorGatewayStatus>>(), thread: vi.fn<(id: string) => Promise<ThreadTunnelGatewayStatus>>(),
  save: vi.fn<(body: GatewayCatalogueUpdateBody) => Promise<GatewayCatalogueUpdated>>(),
  choice: vi.fn<(id:string,body:MachineGatewaySetBody)=>Promise<MachineGatewaySetReceipt>>() }));
vi.mock('@/api/gateways', async (load) => ({ ...await load<typeof GatewayApi>(),
  gatewayCatalogueQuery: { queryKey: ['gateway-catalogue'], queryFn: () => calls.catalogue() },
  machineGatewayQuery: (id: string, online?: boolean) => ({ queryKey: ['machine-gateway', id, online], queryFn: () => calls.machine(id) }),
  threadGatewayQuery: (id: string, revision?: number | null) => ({ queryKey: ['thread-tunnel-gateway', id, revision], queryFn: () => calls.thread(id) }),
  updateGatewayCatalogue: calls.save,
  updateMachineGateway: calls.choice,
}));
beforeEach(() => { vi.clearAllMocks(); calls.catalogue.mockResolvedValue(catalogue); calls.machine.mockResolvedValue(connector);
  calls.thread.mockResolvedValue(tunnel); calls.save.mockResolvedValue({ revision: 'accepted' }); });
afterEach(restore);

it.each(['en','zh-CN'] as const)('saves Automatic on the same enrolled machine and keeps its reported connection while waiting (%s)',async(locale)=>{
  await look({locale,theme:'dark',width:400});
  calls.machine.mockResolvedValue({...connector,selection:'selected',selected_gateway_id:'eu',primary_gateway_id:'eu'});
  let finish:((value:MachineGatewaySetReceipt)=>void)|undefined;
  calls.choice.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
  const user=userEvent.setup();
  show(<MachineGatewayStatus machineId={connector.machine_id} online manage/>);
  await user.click(await screen.findByRole('button',{name:i18n.t('regionalGateways.change')}));
  await user.click(screen.getByRole('combobox',{name:i18n.t('regionalGateways.gateway')}));
  await user.click(await screen.findByRole('option',{name:i18n.t('regionalGateways.automatic')}));
  await user.click(screen.getByRole('button',{name:i18n.t('common.save')}));
  await waitFor(()=>expect(calls.choice).toHaveBeenCalledExactlyOnceWith(connector.machine_id,{selection:'automatic',gateway_id:null}));
  const pending=screen.getByRole('button',{name:i18n.t('regionalGateways.saving')});
  expect(pending).toHaveAttribute('aria-disabled','true');
  await user.dblClick(pending);expect(calls.choice).toHaveBeenCalledOnce();
  expect(screen.getByText(i18n.t('regionalGateways.selected'),{exact:true})).toBeVisible();
  act(()=>finish?.({projection:'receipt',machine_id:connector.machine_id,selection:'automatic',gateway_id:null,changed:true}));
  await waitFor(()=>expect(screen.queryByRole('combobox')).not.toBeInTheDocument());
  expect(screen.getByText(i18n.t('regionalGateways.selected'),{exact:true})).toBeVisible();
  expect(calls.save).not.toHaveBeenCalled();
});

it('refreshes only the actual active gateway projection after a machine report and stays quiet for load',async()=>{
  const cache=new QueryClient(); const id=connector.machine_id; const key=['machine-gateway',id,true] as const;
  const gateway={choice:{selection:'selected',gateway_id:'eu'}};
  cache.setQueryData(['machine',id],{id,online:true,facts:{gateway}});
  cache.setQueryData(key,{...connector,selection:'selected',selected_gateway_id:'eu'});
  const read=vi.fn(()=>Promise.resolve(connector));
  const observer=new QueryObserver(cache,{queryKey:key,queryFn:read,staleTime:Infinity});
  const stop=observer.subscribe(()=>undefined);
  try {
    applyRow(cache,{kind:'machine',id,value:{id,online:true,facts:{gateway,rates:{up_bps:5}}}});
    expect(read).not.toHaveBeenCalled();
    applyRow(cache,{kind:'machine',id,value:{id,online:true,facts:{gateway:{choice:{selection:'automatic'}}}}});
    await waitFor(()=>expect(read).toHaveBeenCalledOnce());
    expect(cache.getQueryData(key)).toEqual(connector);
  } finally {stop();cache.clear();}
});

it('binds the Admin edit to the native ordered replacement and opaque revision', async () => {
  show(<Gateways />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('regionalGateways.edit') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith({ expected_revision: catalogue.revision, gateways: catalogue.gateways }));
});

it('rereads the advertised catalogue for review without writing a replacement', async () => {
  show(<Gateways />);
  await screen.findByRole('button', { name: i18n.t('regionalGateways.edit') });
  expect(screen.queryByText(catalogue.revision, { exact: false })).not.toBeInTheDocument();
  calls.catalogue.mockResolvedValue({ ...catalogue, revision: 'changed-elsewhere' });
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.refresh') }));
  await waitFor(() => expect(calls.catalogue).toHaveBeenCalledTimes(2));
  expect(screen.queryByText('changed-elsewhere', { exact: false })).not.toBeInTheDocument();
  expect(calls.save).not.toHaveBeenCalled();
});

it('uses one machine status identity for personal connectors and Host connector cards', async () => {
  const view = show(<MachineGatewayStatus machineId={connector.machine_id} online />);
  await screen.findByText(i18n.t('regionalGateways.automatic'), { exact: true });
  expect(calls.machine).toHaveBeenCalledExactlyOnceWith(connector.machine_id);
  view.rerender(<MachineGatewayStatus machineId={connector.machine_id} online={false} />);
  await waitFor(() => expect(calls.machine).toHaveBeenCalledTimes(2));
  expect(calls.save).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('links the tunnel report help to its existing article (%s)', async (locale) => {
  await look({ locale, theme: 'dark', width: 400 });
  show(<ThreadGatewayStatus threadId={tunnel.thread_id} />);
  const help = await screen.findByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('regionalGateways.toolConnection') }) });
  expect(help).toHaveAttribute('data-help-article', 'connect-a-machine');
  expect(help).toHaveAttribute('data-help-heading', 'tunnel-gateway');
});

it('refreshes a connector report explicitly without changing its saved selection', async () => {
  show(<MachineGatewayStatus machineId={connector.machine_id} online />);
  await screen.findByText(i18n.t('regionalGateways.automatic'), { exact: true });
  calls.machine.mockResolvedValue({ ...connector, selection: 'selected', selected_gateway_id: 'eu' });
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.refreshConnector') }));
  await screen.findByText(i18n.t('regionalGateways.selected'), { exact: true });
  expect(calls.machine).toHaveBeenCalledTimes(2);
  expect(calls.save).not.toHaveBeenCalled();
});

it('withdraws an old attachment when the current thread source revision changes', async () => {
  let accept: (value: ThreadTunnelGatewayStatus) => void = () => { throw new Error('The new source was not read.'); };
  calls.thread.mockResolvedValueOnce(tunnel).mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }));
  const view = show(<ThreadGatewayStatus threadId={tunnel.thread_id} executionRevision={1} line />);
  await screen.findByText('Asia tools', { exact: true });
  view.rerender(<ThreadGatewayStatus threadId={tunnel.thread_id} executionRevision={2} line />);
  expect(screen.queryByText('Asia tools', { exact: true })).not.toBeInTheDocument();
  await waitFor(() => expect(calls.thread).toHaveBeenCalledTimes(2));
  act(() => { accept({ ...tunnel, state: 'unknown', attached_gateway_id: null }); });
  await waitFor(() => expect(screen.queryByText('Asia tools', { exact: true })).not.toBeInTheDocument());
});

it('does not fan out from an environment thread list before gateway details are opened', async () => {
  show(<ThreadGatewayDetails threadId={tunnel.thread_id} />);
  expect(calls.thread).not.toHaveBeenCalled();
  await userEvent.click(screen.getByText(i18n.t('regionalGateways.toolConnection'), { exact: true }));
  await screen.findByText('Asia tools', { exact: true });
  expect(calls.thread).toHaveBeenCalledExactlyOnceWith(tunnel.thread_id);
});

it('reads a fresh pair when an unreported thread gateway is inspected', async () => {
  calls.thread.mockResolvedValueOnce({ ...tunnel, state: 'unknown', attached_gateway_id: null });
  show(<ThreadGatewayStatus threadId={tunnel.thread_id} executionRevision={1} line />);
  await userEvent.click(await screen.findByText(i18n.t('regionalGateways.toolConnection'), { exact: true }));
  await screen.findByText('Asia tools', { exact: true });
  expect(calls.thread).toHaveBeenCalledTimes(2);
});

it('refreshes an open tunnel report without reopening or replaying work', async () => {
  show(<ThreadGatewayStatus threadId={tunnel.thread_id} executionRevision={1} />);
  await screen.findByText('Asia tools', { exact: true });
  calls.thread.mockResolvedValue({ ...tunnel, state: 'unknown', attached_gateway_id: null });
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.refresh') }));
  await waitFor(() => expect(screen.queryByText('Asia tools', { exact: true })).not.toBeInTheDocument());
  expect(calls.thread).toHaveBeenCalledTimes(2);
  expect(calls.save).not.toHaveBeenCalled();
});

it('treats the catalogue update as a receipt and reads only the active catalogue projection', async () => {
  const cache = new QueryClient();
  const key = ['gateway-catalogue'] as const;
  const next = { ...catalogue, revision: 'next' };
  cache.setQueryData(key, catalogue);
  cache.setQueryData(['unrelated'], { retained: true });
  const read = vi.fn(() => Promise.resolve(next));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  try {
    const act = findAct('PUT', '/gateways', { input: { expected_revision: catalogue.revision, gateways: catalogue.gateways }, answer: { revision: 'next' } });
    expect(act).toBeDefined();
    await applyChanges(act?.changes ?? [], cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toEqual(next);
    expect(cache.getQueryData(['unrelated'])).toEqual({ retained: true });
  } finally { stop(); cache.clear(); }
});
