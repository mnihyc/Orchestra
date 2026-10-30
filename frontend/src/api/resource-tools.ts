/** Served MCP presets and mounts, their revisions, locations and current uses. */
import { queryOptions } from '@tanstack/react-query';
import { api, done, ok, repeatable } from '@/api/client';
import { cachedRow } from '@/api/cache';
import { queryClient } from '@/api/query';
import { resourceRows } from '@/api/listed';
import type { MachineEnvironmentData, MachineMounts, McpServer, McpServerCreateBody, McpOfferingUseBody, McpServerUpdateBody, Mount, MountCreateBody, MountDataKind, MountLocationSetBody, MountUpdateBody, MountUsesGetReply } from '@/generated/api-types';

export const mcpKey = (id: string) => ['mcp-server', id] as const;
export const mountKey = (id: string) => ['mount', id] as const;
export const mcpServersQuery = queryOptions({ queryKey: ['mcp-servers'] as const, queryFn: ({ signal }) => ok(api.GET('/mcp-servers', { signal })).then(resourceRows<McpServer>) });
export const mountsQuery = queryOptions({ queryKey: ['mounts'] as const, queryFn: ({ signal }) => ok(api.GET('/mounts', { signal })).then(resourceRows<Mount>) });
export const machineMountsKey = (machine: string) => ['mounts', 'machine', machine] as const;
export const machineEnvironmentDataQuery = (machine: string) => queryOptions({ queryKey: ['machine-environment-data', machine] as const, queryFn: ({ signal }): Promise<MachineEnvironmentData> => ok(api.GET('/machines/{id}/environment-data', { params: { path: { id: machine } }, signal })) });
export const machineMountsQuery = (machine: string) => queryOptions({ queryKey: machineMountsKey(machine), queryFn: ({ signal }): Promise<MachineMounts> => ok(api.GET('/machines/{id}/mounts', { params: { path: { id: machine } }, signal })) });
export const mcpQuery = (id: string) => queryOptions({ queryKey: mcpKey(id), initialData: () => cachedRow(queryClient, 'mcp_server', id), queryFn: ({ signal }): Promise<McpServer> => ok(api.GET('/mcp-servers/{id}', { params: { path: { id } }, signal })) });
export const mountQuery = (id: string) => queryOptions({ queryKey: mountKey(id), initialData: () => cachedRow(queryClient, 'mount', id), queryFn: ({ signal }): Promise<Mount> => ok(api.GET('/mounts/{id}', { params: { path: { id } }, signal })) });
export const mountUsesQuery = (id: string) => queryOptions({ queryKey: ['mount-uses', id] as const, queryFn: ({ signal }): Promise<MountUsesGetReply> => ok(api.GET('/mounts/{id}/uses', { params: { path: { id } }, signal })) });
export const createMcp = (body: McpServerCreateBody, key: string): Promise<McpServer> => ok(api.POST('/mcp-servers', { body, ...repeatable(key) }));
export const updateMcp = (id: string, body: McpServerUpdateBody): Promise<McpServer> => ok(api.PATCH('/mcp-servers/{id}', { params: { path: { id } }, body }));
export const reimportMcp = (id: string): Promise<McpServer> => ok(api.POST('/mcp-servers/{id}/reimport', { params: { path: { id } } }));
export const deleteMcp = (id: string): Promise<void> => done(api.DELETE('/mcp-servers/{id}', { params: { path: { id } } }));
export const createMount = (body: MountCreateBody, key: string): Promise<Mount> => ok(api.POST('/mounts', { body, ...repeatable(key) }));
export const updateMount = (id: string, body: MountUpdateBody): Promise<Mount> => ok(api.PATCH('/mounts/{id}', { params: { path: { id } }, body }));
export const deleteMount = (id: string): Promise<void> => done(api.DELETE('/mounts/{id}', { params: { path: { id } } }));
export const setMountLocation = (id: string, machine: string, body: MountLocationSetBody): Promise<Mount> => ok(api.PUT('/mounts/{id}/locations/{machine_id}', { params: { path: { id, machine_id: machine } }, body }));
export const setMountLocationDataKind = (id: string, machine: string, dataKind: MountDataKind | null): Promise<Mount> => ok(api.PATCH('/mounts/{id}/locations/{machine_id}', { params: { path: { id, machine_id: machine } }, body: { data_kind: dataKind } }));
export const purgeKeptMountData = (id: string, kept: string, confirm?: string): Promise<void> => done(api.DELETE('/mounts/{id}/kept/{kept_id}', { params: { path: { id, kept_id: kept }, query: confirm === undefined ? {} : { confirm } } }));
export const removeMountLocation = (id: string, machine: string, confirm?: string): Promise<void> => done(api.DELETE('/mounts/{id}/locations/{machine_id}', { params: { path: { id, machine_id: machine }, query: confirm === undefined ? {} : { confirm } } }));

export const selectMcpOffering = (id: string, machine_id: string, body: McpOfferingUseBody): Promise<McpServer> => ok(api.POST('/mcp-servers/{id}/offerings/{machine_id}/use', { params: { path: { id, machine_id } }, body }));
