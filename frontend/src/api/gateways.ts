/** Gateway metadata and source-bound reports use their existing native read authorities. */
import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { accepted, api, ok } from '@/api/client';
import type { ConnectorGatewaySelection, ConnectorGatewayStatus, GatewayCatalogue, GatewayCatalogueUpdateBody, GatewayCatalogueUpdated, ThreadTunnelGatewayStatus, MachineGatewaySetBody, MachineGatewaySetReceipt } from '@/generated/api-types';

export const gatewayCatalogueKey = ['gateway-catalogue'] as const;
export const gatewayCatalogueQuery = queryOptions({
  queryKey: gatewayCatalogueKey,
  queryFn: ({ signal }): Promise<GatewayCatalogue> => ok(api.GET('/gateways', { signal })),
});
export function updateGatewayCatalogue(body: GatewayCatalogueUpdateBody): Promise<GatewayCatalogueUpdated> {
  return ok(api.PUT('/gateways', { body }));
}
export const machineGatewayKey = (id: string) => ['machine-gateway', id] as const;
export function updateMachineGateway(machineId:string,body:MachineGatewaySetBody):Promise<MachineGatewaySetReceipt> {
  return accepted(api.PUT('/machines/{machine_id}/gateway',{params:{path:{machine_id:machineId}},body}));
}
/**
 * A saved primary choice. Its receipt comes after the connector has saved the choice, so every shown status of that
 * machine takes it at once; a read at that moment would only bring the connector's report from before, which the
 * connector replaces once it has joined its new primary (the machine's row then tells the status to read again).
 */
export interface MachineGatewayChoice { machineGateway: { machine: string; selection: ConnectorGatewaySelection; gatewayId: string | null } }

/** The receipt of a primary choice of `machine`, or `null` for an answer that is not one. */
export function machineGatewayChoice(machine: string, answer: unknown): MachineGatewayChoice | null {
  if (typeof answer !== 'object' || answer === null) return null;
  const { projection, machine_id: id, selection, gateway_id: gatewayId } = answer as Partial<MachineGatewaySetReceipt>;
  if (projection !== 'receipt' || id !== machine || (selection !== 'automatic' && selection !== 'selected')) return null;
  if (selection === 'selected' ? typeof gatewayId !== 'string' : gatewayId !== null && gatewayId !== undefined) return null;
  return { machineGateway: { machine, selection, gatewayId: selection === 'selected' ? gatewayId ?? null : null } };
}

/** Writes a saved choice into every held status of its machine, after the reads under way are stopped. */
export async function applyMachineGatewayChoice(cache: QueryClient, choice: MachineGatewayChoice['machineGateway']): Promise<void> {
  const filter = { queryKey: machineGatewayKey(choice.machine) };
  await cache.cancelQueries(filter);
  cache.setQueriesData<ConnectorGatewayStatus>(filter, (held) => held === undefined ? held
    : { ...held, selection: choice.selection, selected_gateway_id: choice.gatewayId });
}

export function machineGatewayQuery(machineId: string, online?: boolean) {
  return queryOptions({
    queryKey: [...machineGatewayKey(machineId), online],
    queryFn: ({ signal }): Promise<ConnectorGatewayStatus> => ok(api.GET('/machines/{machine_id}/gateway', { params: { path: { machine_id: machineId } }, signal })),
  });
}
export const threadGatewayKey = (id: string) => ['thread-tunnel-gateway', id] as const;
export function threadGatewayQuery(threadId: string, executionRevision?: number | null) {
  return queryOptions({
    queryKey: [...threadGatewayKey(threadId), executionRevision],
    queryFn: ({ signal }): Promise<ThreadTunnelGatewayStatus> => ok(api.GET('/threads/{thread_id}/gateway', { params: { path: { thread_id: threadId } }, signal })),
  });
}
