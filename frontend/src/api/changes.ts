/** API client for changing what an environment, a provider's runtime or a host is: update and recreate, move to another host, and recycle.
 *
 * The same four views of a plan, a confirmation, a job and its end as a fork or a switch has, for the things
 * that are not a thread: the plan of one environment or of several at a place, the plan of a provider's
 * runtime or of several, carrying a plan out (the job it makes, or the batch of jobs), and recycling what is
 * idle on a host or at a place, first as a dry run that stops nothing and then for real.
 *
 * What is asked of one thing is asked of its own path; what is asked of several is asked of a place
 * (`/nodes/{id}/environments/...`) or of the providers' runtimes together. Carrying out names the plan, and
 * `ForkCreateBody` is the one body every kind of plan is carried out with. What the screens ask today is one
 * environment's update and move, the recycle of a host and of a place, and the step that opens a provider to
 * those who go on talking to a thread; the plans of several environments and of a provider's runtime are
 * written ahead, for the screens that will list them, and no screen asks them yet.
 */
import { api, ok, repeatable } from '@/api/client';
import { queryOptions } from '@tanstack/react-query';
import type {
  EnvironmentBatch,
  EnvironmentMoveBatchPlan,
  EnvironmentMovePlan,
  EnvironmentMovePreflightBody,
  EnvironmentUpdateBatchPlan,
  EnvironmentUpdatePlan,
  ForkCreateBody,
  Job,
  ProviderBatch,
  ProviderRuntimeUpdateBatchPreflightReply,
  ProviderUpdatePlan,
  PermissionPage,
  Dependency,
  RecycleBody,
  RecycleReply,
  UpdatePreflightReason,
} from '@/generated/api-types';

// ---------------------------------------------------------------- an environment

/** What updating one environment, or only recreating it, would do. */
export function planEnvironmentUpdate(id: string, reason: UpdatePreflightReason, signal?: AbortSignal): Promise<EnvironmentUpdatePlan> {
  return ok(api.POST('/environments/{id}/updates/preflight', { params: { path: { id } }, body: { reason }, ...(signal === undefined ? {} : { signal }) }));
}

/** Carries the plan out: the environment is `recreating` and every thread in it is locked from now on. */
export function startEnvironmentUpdate(id: string, body: ForkCreateBody, key: string, notify = true): Promise<Job> {
  return ok(api.POST('/environments/{id}/updates', { params: { path: { id }, query: { notify } }, body, ...repeatable(key) }));
}

/** The permitted machines and their declared move services and policy refusals. */
export const environmentMovePlacesQuery = (id: string, templateId: string | null = null) => queryOptions({
  queryKey: ['environment-move-places', id, templateId],
  queryFn: ({ signal }) => ok(api.GET('/environments/{id}/moves/places', { params: { path: { id }, query: templateId === null ? {} : { template_id: templateId } }, signal })),
});

/** What moving one environment's owned data to a permitted machine would do. */
export function planEnvironmentMove(id: string, body: EnvironmentMovePreflightBody, signal?: AbortSignal): Promise<EnvironmentMovePlan> {
  return ok(api.POST('/environments/{id}/moves/preflight', { params: { path: { id } }, body, ...(signal === undefined ? {} : { signal }) }));
}

/** Carries the plan of a move out. */
export function startEnvironmentMove(id: string, body: ForkCreateBody, key: string): Promise<Job> {
  return ok(api.POST('/environments/{id}/moves', { params: { path: { id } }, body, ...repeatable(key) }));
}

// ---------------------------------------------------------------- several environments at a place

/** What updating several environments of a place would do: one plan each. */
export function planNodeUpdates(nodeId: string, environmentIds: string[], signal?: AbortSignal): Promise<EnvironmentUpdateBatchPlan> {
  return ok(api.POST('/nodes/{id}/environments/updates/preflight', { params: { path: { id: nodeId } }, body: { environment_ids: environmentIds }, ...(signal === undefined ? {} : { signal }) }));
}

/** Carries the batch of updates out. */
export function startNodeUpdates(nodeId: string, body: ForkCreateBody, key: string, notify = true): Promise<EnvironmentBatch> {
  return ok(api.POST('/nodes/{id}/environments/updates', { params: { path: { id: nodeId }, query: { notify } }, body, ...repeatable(key) }));
}

/** One reviewed batch may contain environments from different hosts and homes. */
export function planEnvironmentUpdates(environmentIds: string[], signal?: AbortSignal): Promise<EnvironmentUpdateBatchPlan> {
  return ok(api.POST('/environments/updates/preflight', { body: { environment_ids: environmentIds }, ...(signal === undefined ? {} : { signal }) }));
}
export function startEnvironmentUpdates(body: ForkCreateBody, key: string, notify = true): Promise<EnvironmentBatch> {
  return ok(api.POST('/environments/updates', { params: { query: { notify } }, body, ...repeatable(key) }));
}

/** What moving several environments to one host would do. */
export function planNodeMoves(nodeId: string, environmentIds: string[], hostId: string | null, signal?: AbortSignal, destination?: Pick<EnvironmentMovePreflightBody, 'machine_id' | 'template_id' | 'copy_directories'>): Promise<EnvironmentMoveBatchPlan> {
  return ok(
    api.POST('/nodes/{id}/environments/moves/preflight', {
      params: { path: { id: nodeId } },
      body: { environment_ids: environmentIds, ...(hostId === null ? {} : { host_id: hostId }), ...destination },
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** Carries the batch of moves out. */
export function startNodeMoves(nodeId: string, body: ForkCreateBody, key: string): Promise<EnvironmentBatch> {
  return ok(api.POST('/nodes/{id}/environments/moves', { params: { path: { id: nodeId } }, body, ...repeatable(key) }));
}

// ---------------------------------------------------------------- a provider's runtime

/** What updating the runtime of one provider would do. */
export function planProviderUpdate(id: string, reason: UpdatePreflightReason, signal?: AbortSignal): Promise<ProviderUpdatePlan> {
  return ok(api.POST('/providers/{id}/runtime/updates/preflight', { params: { path: { id } }, body: { reason }, ...(signal === undefined ? {} : { signal }) }));
}

/** Carries the plan out: every thread bound to the provider stands still until it is done. */
export function startProviderUpdate(id: string, body: ForkCreateBody, key: string, notify = true): Promise<Job> {
  return ok(api.POST('/providers/{id}/runtime/updates', { params: { path: { id }, query: { notify } }, body, ...repeatable(key) }));
}

/** What updating the runtimes of several providers would do. */
export function planProviderUpdates(providerIds: string[], signal?: AbortSignal): Promise<ProviderRuntimeUpdateBatchPreflightReply> {
  return ok(api.POST('/providers/runtime/updates/preflight', { body: { provider_ids: providerIds }, ...(signal === undefined ? {} : { signal }) }));
}

/** Carries the batch of runtime updates out. */
export function startProviderUpdates(body: ForkCreateBody, key: string, notify = true): Promise<ProviderBatch> {
  return ok(api.POST('/providers/runtime/updates', { params: { query: { notify } }, body, ...repeatable(key) }));
}

// ---------------------------------------------------------------- recycle

/** Stops what is idle at a place, or only says what it would stop (`dry_run`): the exact list, and why the rest is left. */
export function recycleNode(nodeId: string, body: RecycleBody): Promise<RecycleReply> {
  return ok(api.POST('/nodes/{id}/environments/recycle', { params: { path: { id: nodeId } }, body }));
}

/** The same for everything that runs on a host, for the people who manage it. */
export function recycleHost(hostId: string, body: RecycleBody): Promise<RecycleReply> {
  return ok(api.POST('/hosts/{id}/recycle', { params: { path: { id: hostId } }, body }));
}

// ---------------------------------------------------------------- the step that opens a provider to the work

/** The collection a kind of thing is addressed under, for the permission page that every kind has. */
const COLLECTIONS: Record<string, string> = { provider: 'providers', environment: 'environments', agent: 'agents', host: 'hosts', template: 'templates' };

/**
 * Opens what a thread is to run on to the people who work where the thread is: the place gets the entry that the
 * plan named (`through`) in the row the dependency is about (`provider.use`). It is the one change of a page that
 * a switch takes in its own dialog, by whoever manages that page.
 */
export function openDependency(dependency: Dependency): Promise<PermissionPage> {
  const through = dependency.through;
  if (through === null) return Promise.reject(new Error('there is no place to open it to'));
  return ok(
    api.PATCH('/{object}/{id}/permissions', {
      params: { path: { object: COLLECTIONS[dependency.object.kind] ?? `${dependency.object.kind}s`, id: dependency.object.id } },
      body: { rows: [{ permission: dependency.permission, add: [{ holder: { kind: 'node', id: through.id }, ...(dependency.options === null || dependency.options === undefined ? {} : { options: dependency.options }) }] }] },
    }),
  );
}
