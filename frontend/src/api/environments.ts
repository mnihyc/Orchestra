/** API client for Environments as resources: creation, lifecycle, configuration changed while one runs, what the exec node offers, MCP servers found, imported and of its own, its history, updates, scheduled removal and moves.
 *
 * So far: the environments a person may see or manage, one with the threads in it and where it comes
 * from, its own settings with where each value comes from, a directory on a person's machine registered
 * as an environment, one made from a template, starting and stopping a made one and keeping it awake, and
 * its name, its switches, its being switched off and its removal. Updating and moving it and its Tools are
 * not asked for here.
 */
import { queryOptions } from '@tanstack/react-query';

import { collectionQuery, readAllPages } from '@/api/collection';
import { accepted, api, done, ok, repeatable } from '@/api/client';
import { cachedRow, feedSequences } from '@/api/cache';
import { agentReachKey } from '@/api/agentReach';
import { queryClient, type ReadOptions } from '@/api/query';
import { type ResourceRows, resourceRows } from '@/api/listed';
import { libraryReader, sameLibraryReader, seedEnvironmentLibrary } from '@/api/resourceLibrary';
import type {
  AgentReachOverview,
  Environment,
  EnvironmentMcp,
  EnvironmentMcpImportCreateBody,
  EnvironmentUpdateBody,
  Job,
  MachineEnvironmentCreateBody,
  NewEnvironment,
  ResolvedSetting,
} from '@/generated/api-types';

export const environmentsKey = ['environments'] as const;
export const environmentKey = (id: string) => ['environment', id] as const;
export const environmentSettingsKey = (id: string) => ['environment-settings', id] as const;
export const environmentMcpKey = (id: string, threadId?: string) => threadId === undefined ? ['environment-mcp', id] as const : ['environment-mcp', id, threadId] as const;

/** A visible child directory uses the same native whole/through-thread/retained read authority. */
export const environmentThreadsPageQuery = (id: string, q: string, version = '') => collectionQuery(['environment-threads', id, version], q,
  async (query, signal) => {
    const reply = await ok(api.GET('/environments/{id}', { params: { path: { id }, query: {
      threads_q: query.q, threads_limit: query.limit, ...(query.cursor === undefined ? {} : { threads_cursor: query.cursor }),
    } }, signal }));
    return { items: reply.threads, next_cursor: reply.threads_next_cursor ?? null, hidden: reply.threads_hidden };
  });

/** Current hosted server states and masked failures. */
export const environmentMcpPageQuery = (id: string, collection: 'servers' | 'found', q: string, threadId?: string) => collectionQuery([...environmentMcpKey(id, threadId), collection], q,
  async (query, signal) => {
    const reply = await ok(api.GET('/environments/{id}/mcp', { params: { path: { id }, query: { ...query, collection, ...(threadId === undefined ? {} : { thread_id: threadId }) } }, signal }));
    return { ...reply, items: collection === 'found' ? reply.found : reply.servers };
  });

/** Bulk import/configuration needs every admitted definition, independently of the visible page. */
export const environmentMcpQuery = (id: string, threadId?: string) => queryOptions({ queryKey: [...environmentMcpKey(id, threadId), 'complete'], queryFn: async ({ signal }): Promise<EnvironmentMcp> => {
  let facts: EnvironmentMcp | undefined;
  const connections = new Map<string, NonNullable<EnvironmentMcp['connections']>[number]>();
  const read = async (collection: 'servers' | 'found', cursor: string | null) => {
    const reply = await ok(api.GET('/environments/{id}/mcp', { params: { path: { id }, query: { collection, ...(threadId === undefined ? {} : { thread_id: threadId }), ...(cursor === null ? {} : { cursor }) } }, signal }));
    if (collection === 'found') for (const row of reply.connections ?? []) connections.set(JSON.stringify([row.source, row.name]), row);
    facts = reply;
    return reply;
  };
  const servers = await readAllPages(async (cursor) => { const reply = await read('servers', cursor); return { items: reply.servers, next_cursor: reply.next_cursor }; }, signal);
  const found = await readAllPages(async (cursor) => { const reply = await read('found', cursor); return { items: reply.found, next_cursor: reply.next_cursor }; }, signal);
  if (facts === undefined) throw new Error('The native catalogue did not return its facts.');
  return { ...facts, servers, found, connections: [...connections.values()], next_cursor: null };
} });

/** Import and optional enablement are one producer, including an accepted restricted receipt. */
export async function importEnvironmentMcp(id: string, body: EnvironmentMcpImportCreateBody, key: string) {
  const reply = await accepted(api.POST('/environments/{id}/mcp/imports', { params: { path: { id } }, body, ...repeatable(key) }));
  if ('projection' in reply) return { id: reply.server_id, server: null };
  return { id: reply.id, server: reply };
}

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
      const reader = libraryReader();
      const signal = options.keepReading === true ? undefined : context.signal;
      const snapshot = await ok(api.GET('/environments/{id}/sync', { params: { path: { id } }, ...(signal === undefined ? {} : { signal }) }));
      if (signal?.aborted !== true && sameLibraryReader(reader) && snapshot.environment.id === id) seedEnvironmentLibrary(context.client, id, snapshot.library);
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

/** An omitted home is selected from the registered Machine in the guarded native creation. */
export type MachineEnvironmentRegistration = Omit<MachineEnvironmentCreateBody, 'home_id'> & {home_id?: string};

/** Registers a directory on a machine of the caller's as an environment. The machine checks the root against its own policy. */
export function registerEnvironment(machineId: string, body: MachineEnvironmentRegistration, key: string) {
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
