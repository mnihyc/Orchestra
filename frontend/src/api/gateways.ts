/** Gateway metadata and source-bound reports use their existing native read authorities. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { ConnectorGatewayStatus, GatewayCatalogue, GatewayCatalogueUpdateBody, GatewayCatalogueUpdated, ThreadTunnelGatewayStatus } from '@/generated/api-types';

export const gatewayCatalogueKey = ['gateway-catalogue'] as const;
export const gatewayCatalogueQuery = queryOptions({
  queryKey: gatewayCatalogueKey,
  queryFn: ({ signal }): Promise<GatewayCatalogue> => ok(api.GET('/gateways', { signal })),
});
export function updateGatewayCatalogue(body: GatewayCatalogueUpdateBody): Promise<GatewayCatalogueUpdated> {
  return ok(api.PUT('/gateways', { body }));
}
export const machineGatewayKey = (id: string) => ['machine-gateway', id] as const;
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
