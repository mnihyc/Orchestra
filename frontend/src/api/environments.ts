/** API client for Environments as resources: creation, lifecycle, configuration changed while one runs, what the exec node offers, MCP servers found, imported and of its own, its history, updates, scheduled removal and moves.
 *
 * So far: the environments a person may see or manage, one with the threads in it and where it comes
 * from, its own settings with where each value comes from, a directory on a person's machine registered
 * as an environment, one made from a template, starting and stopping a made one and keeping it awake, and
 * its name, its switches, its being switched off and its removal. Updating and moving it and its Tools are
 * not asked for here.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import { cachedRow, feedSequences } from '@/api/cache';
import { agentReachKey } from '@/api/agentReach';
import { queryClient, type ReadOptions } from '@/api/query';
import { type ResourceRows, resourceRows } from '@/api/listed';
import type {
  AgentReachOverview,
  Environment,
  EnvironmentMcp,
  EnvironmentUpdateBody,
  Job,
  MachineEnvironmentCreateBody,
  NewEnvironment,
  ResolvedSetting,
} from '@/generated/api-types';

export const environmentsKey = ['environments'] as const;
export const environmentKey = (id: string) => ['environment', id] as const;
export const environmentSettingsKey = (id: string) => ['environment-settings', id] as const;
export const environmentMcpKey = (id: string) => ['environment-mcp', id] as const;

/** Current hosted server states and masked failures. */
export const environmentMcpQuery = (id: string) => queryOptions({ queryKey: environmentMcpKey(id), queryFn: ({ signal }): Promise<EnvironmentMcp> => ok(api.GET('/environments/{id}/mcp', { params: { path: { id } }, signal })) });

/** Restart only this server, as a job; environment files and running turns stay in place. */
export function restartEnvironmentMcpServer(id: string, name: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/mcp/servers/{name}/restart', { params: { path: { id, name } } }));
}

/** The environments the caller may use or manage; for an org, those whose home lies in it. */
export const environmentsQuery = (orgId: string | null) =>
  queryOptions({
    queryKey: [...environmentsKey, orgId] as const,
    queryFn: ({ signal }): Promise<ResourceRows<Environment>> =>
      ok(api.GET('/environments', { params: { query: orgId === null ? {} : { org_id: orgId } }, signal })).then(resourceRows<Environment>),
  });

export const environmentQuery = (id: string, options: ReadOptions = {}) =>
  queryOptions({
    queryKey: environmentKey(id),
    initialData: () => cachedRow(queryClient, 'environment', id),
    queryFn: async (context): Promise<Environment> => {
      const snapshot = await ok(api.GET('/environments/{id}/sync', { params: { path: { id } }, ...(options.keepReading === true ? {} : { signal: context.signal }) }));
      feedSequences.set(`env:${id}`, snapshot.seq);
      return snapshot.environment;
    },
  });

/** The caller's cut of the network, secret names, mounts, per-thread key placement and browser profiles. */
export const environmentReachQuery = (id: string) =>
  queryOptions({
    queryKey: agentReachKey(id),
    queryFn: ({ signal }): Promise<AgentReachOverview> =>
      ok(api.GET('/environments/{id}/agent-reach', { params: { path: { id } }, signal })),
  });

/** The values in effect for an environment, each with the place they come from and whether this caller can change them. */
export const environmentSettingsQuery = (id: string) =>
  queryOptions({
    queryKey: environmentSettingsKey(id),
    queryFn: ({ signal }): Promise<ResolvedSetting[]> =>
      ok(api.GET('/environments/{id}/settings', { params: { path: { id } }, signal })),
  });

/** Registers a directory on a machine of the caller's as an environment. The machine checks the root against its own policy. */
export function registerEnvironment(machineId: string, body: MachineEnvironmentCreateBody, key: string): Promise<Environment> {
  return ok(api.POST('/machines/{id}/environments', { params: { path: { id: machineId } }, body, ...repeatable(key) }));
}

/** Changes an environment on the revision the person saw. */
export function updateEnvironment(id: string, body: EnvironmentUpdateBody): Promise<Environment> {
  return ok(api.PATCH('/environments/{id}', { params: { path: { id } }, body }));
}

export function disableEnvironment(id: string,note?:string): Promise<Environment> {
  return ok(api.POST('/environments/{id}/disable', { params: { path: { id },...(note===undefined?{}:{query:{note}}) } }));
}

export function enableEnvironment(id: string,runWaiting=false): Promise<Environment> {
  return ok(api.POST('/environments/{id}/enable', { params: { path: { id }, query:{run_waiting:runWaiting} } }));
}

/** Removes an environment, naming it to be sure. A directory on a person's machine is only forgotten. */
export function deleteEnvironment(id: string, confirm: string): Promise<void> {
  return done(api.DELETE('/environments/{id}', { params: { path: { id }, query: { confirm } } }));
}

/** Makes an environment from a template on a host; it is being made, by a job, when the answer comes. */
export function createEnvironment(body: NewEnvironment, key: string): Promise<Environment> {
  return ok(api.POST('/environments', { body, ...repeatable(key) }));
}

/** Starts a stopped environment that Orch made; the job that does it is the answer. */
export function startEnvironment(id: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/start', { params: { path: { id } } }));
}

/** Stops an environment that Orch made, interrupting the turns that run in it only when asked to. */
export function stopEnvironment(id: string, interrupt: boolean): Promise<Environment> {
  return ok(api.POST('/environments/{id}/stop', { params: { path: { id } }, body: { interrupt } }));
}

/** Restarts the same managed unit, preserving files and queued input order. */
export function restartEnvironment(id:string,mode:'wait'|'interrupt'):Promise<Job> {
  return ok(api.POST('/environments/{id}/restart',{params:{path:{id}},body:{mode}}));
}

/** Asks that the environment not be stopped for being idle, as far as its owner allows. */
export function keepEnvironmentAwake(id: string): Promise<Environment> {
  return ok(api.POST('/environments/{id}/keep-awake', { params: { path: { id } } }));
}

/** Makes a temporary environment lasting, withdrawing its scheduled removal. */
export function keepEnvironment(id: string): Promise<Environment> {
  return ok(api.POST('/environments/{id}/keep', { params: { path: { id } } }));
}

/** Captures the complete workspace as an owner-only downloadable snapshot, without a thread. */
export function exportEnvironmentFiles(id: string, key: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/files/export', { params: { path: { id } }, ...repeatable(key) }));
}

/** Downloads a completed immutable copy, including after the source environment is removed. */
export function downloadEnvironmentExport(id: string): Promise<Blob> {
  return ok(api.GET('/snapshots/{id}/archive', { params: { path: { id }, query: { format: 'zip' } }, parseAs: 'blob' }));
}
