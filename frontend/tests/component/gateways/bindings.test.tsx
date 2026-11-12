/** Native reads keep machine and thread identities distinct; catalogue receipts repair one read. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as GatewayApi from '@/api/gateways';
import { applyChanges, findAct } from '@/api/acts';
import { Gateways } from '@/features/admin/Gateways';
import { MachineGatewayStatus } from '@/features/gateways/MachineGatewayStatus';
import { ThreadGatewayDetails, ThreadGatewayStatus } from '@/features/gateways/ThreadGatewayStatus';
import type { ConnectorGatewayStatus, GatewayCatalogue, GatewayCatalogueUpdateBody, GatewayCatalogueUpdated, ThreadTunnelGatewayStatus } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from '../support';
import { catalogue, connector, look, restore, tunnel } from './fixtures';

const calls = vi.hoisted(() => ({ catalogue: vi.fn<() => Promise<GatewayCatalogue>>(),
  machine: vi.fn<(id: string) => Promise<ConnectorGatewayStatus>>(), thread: vi.fn<(id: string) => Promise<ThreadTunnelGatewayStatus>>(),
  save: vi.fn<(body: GatewayCatalogueUpdateBody) => Promise<GatewayCatalogueUpdated>>() }));
vi.mock('@/api/gateways', async (load) => ({ ...await load<typeof GatewayApi>(),
  gatewayCatalogueQuery: { queryKey: ['gateway-catalogue'], queryFn: () => calls.catalogue() },
  machineGatewayQuery: (id: string, online?: boolean) => ({ queryKey: ['machine-gateway', id, online], queryFn: () => calls.machine(id) }),
  threadGatewayQuery: (id: string, revision?: number | null) => ({ queryKey: ['thread-tunnel-gateway', id, revision], queryFn: () => calls.thread(id) }),
  updateGatewayCatalogue: calls.save,
}));
beforeEach(() => { vi.clearAllMocks(); calls.catalogue.mockResolvedValue(catalogue); calls.machine.mockResolvedValue(connector);
  calls.thread.mockResolvedValue(tunnel); calls.save.mockResolvedValue({ revision: 'accepted' }); });
afterEach(restore);

it('binds the Admin edit to the native ordered replacement and opaque revision', async () => {
  show(<Gateways />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('regionalGateways.edit') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith({ expected_revision: catalogue.revision, gateways: catalogue.gateways }));
});

it('rereads the advertised catalogue for review without writing a replacement', async () => {
  show(<Gateways />);
  await screen.findByText(i18n.t('regionalGateways.revision', { revision: catalogue.revision }));
  calls.catalogue.mockResolvedValue({ ...catalogue, revision: 'changed-elsewhere' });
  await userEvent.click(screen.getByRole('button', { name: i18n.t('regionalGateways.refresh') }));
  await screen.findByText(i18n.t('regionalGateways.revision', { revision: 'changed-elsewhere' }));
  expect(calls.catalogue).toHaveBeenCalledTimes(2);
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
