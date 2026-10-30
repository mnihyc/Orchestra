/** Machine-local program snapshots and one acknowledged native operation, without follow-up reads or polling. */
import { queryOptions } from '@tanstack/react-query';
import { api,ok } from '@/api/client';
import type { MachinePrograms,MachineProgramsCommand } from '@/generated/api-types';

export const machineProgramsKey=(id:string)=>['machine-programs',id] as const;
export const machineProgramsQuery=(id:string)=>queryOptions({queryKey:machineProgramsKey(id),
  queryFn:({signal}):Promise<MachinePrograms>=>ok(api.GET('/machines/{id}/programs',{params:{path:{id}},signal}))});

/** The full updated registry comes back from the machine; callers write this answer into its cache. */
export function changeMachinePrograms(id:string,body:MachineProgramsCommand):Promise<MachinePrograms> {
  return ok(api.POST('/machines/{id}/programs',{params:{path:{id}},body}));
}
