/** On-intent native folder choices, before a machine directory becomes an environment. */
import { api, ok } from '@/api/client';
import type { MachineDirectoryListQuery, MachineDirectoryPage } from '@/generated/api-types';

export function listMachineDirectories(machine: string, query: MachineDirectoryListQuery, signal?: AbortSignal): Promise<MachineDirectoryPage> {
  return ok(api.GET('/machines/{id}/directories', { params: { path: { id: machine }, query }, ...(signal === undefined ? {} : { signal }) }));
}
