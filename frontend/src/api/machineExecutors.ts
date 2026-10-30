/** Executors and tool sets reported for this machine, read only when its offers are edited. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { MachineExecutor } from '@/generated/api-types';

export const machineExecutorsKey = (id: string) => ['machine-executors', id] as const;
export const machineExecutorsQuery = (id: string) => queryOptions({
  queryKey: machineExecutorsKey(id),
  queryFn: ({ signal }): Promise<MachineExecutor[]> => ok(api.GET('/machines/{id}/executors', { params: { path: { id } }, signal })),
});
