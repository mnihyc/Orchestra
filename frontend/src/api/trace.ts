/** Audit trails, filtered files and retained things through their current authorized readers. */
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, ok, repeatable } from '@/api/client';
import { jobQuery } from '@/api/exports';
import type { TraceSearch, TraceScope } from '@/features/trace/params';
import { timestamp } from '@/features/trace/params';
import type { KeptDocumentRef, PendingTransport } from '@/features/trace/pending';
import type { AdminUserPage, AuditExportFormat, AuditKind, Document, Job, Node, DeletedEnvironment, Environment as RetainedEnvironment, AuditEntryPage as TracePage } from '@/generated/api-types';

/** A page fits a short trail without obscuring continuation in a long one. */
export const TRAIL_PAGE_SIZE = 40;
export const traceKey = (scope: TraceScope) => ['trace', scope.type === 'system' ? 'system' : scope.id] as const;
export function wireFilters(filters: TraceSearch) {
  return { ...(filters.actor ? { actor: filters.actor } : {}), ...(filters.include_agents !== undefined ? { include_agents: String(filters.include_agents) } : {}), ...(filters.action ? { action: filters.action } : {}), ...(filters.kind ? { kind: filters.kind } : {}), ...(filters.from ? { from: timestamp(filters.from) } : {}), ...(filters.to ? { to: timestamp(filters.to, true) } : {}) };
}
/** The same filters as the typed query of the trail asks them; a kind the server does not know it refuses. */
function typedFilters(filters: TraceSearch) {
  return { ...(filters.actor ? { actor: filters.actor } : {}), ...(filters.include_agents !== undefined ? { include_agents: filters.include_agents } : {}), ...(filters.action ? { action: filters.action } : {}), ...(filters.kind ? { kind: filters.kind as AuditKind } : {}), ...(filters.from ? { from: timestamp(filters.from) } : {}), ...(filters.to ? { to: timestamp(filters.to, true) } : {}) };
}
export const trailQuery = (scope: TraceScope, filters: TraceSearch) => infiniteQueryOptions({
  ...(scope.type === 'system' ? ADMIN_QUERY : {}),
  queryKey: [...traceKey(scope), wireFilters(filters)], initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<TracePage> => {
    const query = { ...typedFilters(filters), limit: TRAIL_PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) };
    return scope.type === 'system'
      ? ok(api.GET('/admin/audit', { params: { query }, signal }))
      : ok(api.GET('/nodes/{id}/audit', { params: { path: { id: scope.id }, query }, signal }));
  }, getNextPageParam: (page) => page.next_cursor,
  // Every direct visit rechecks authority before old content is shown.
  staleTime: 0, gcTime: 0,
});

/** Pending routes use the ordinary bearer and safe error shape, with the same token renewal as the typed client. */
export async function traceRequest(path: string, options: { method?: 'GET' | 'POST' | 'DELETE'; body?: unknown; signal?: AbortSignal; key?: string; headers?: HeadersInit } = {}): Promise<Response> {
  const transport = api as unknown as PendingTransport;
  const answer = await transport.request((options.method ?? 'GET').toLowerCase(), path, { parseAs: 'stream', ...(options.headers === undefined ? {} : { headers: options.headers }), ...(options.body === undefined ? {} : { body: options.body }), ...(options.signal === undefined ? {} : { signal: options.signal }), ...(options.key === undefined ? {} : repeatable(options.key)) });
  await ok(Promise.resolve(answer));
  return answer.response;
}
export async function traceJson<T>(path: string, options?: Parameters<typeof traceRequest>[1]): Promise<T> {
  return (await traceRequest(path, options)).json() as Promise<T>;
}
export const systemUsersQuery = (q: string) => infiniteQueryOptions({
  ...ADMIN_QUERY,
  queryKey: ['trace-users', q], initialPageParam: null as string | null,
  queryFn: ({ signal, pageParam }): Promise<AdminUserPage> => ok(api.GET('/admin/users', { params: { query: { ...(q ? { q } : {}), limit: TRAIL_PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal })),
  getNextPageParam: (page) => page.next_cursor,
});
export const selectedPersonQuery = (id: string, orgId?: string) => queryOptions({ queryKey: ['trace-person', id, orgId], queryFn: async ({ signal }) => {
  return ok(api.GET('/users/{id}', { params: { path: { id } }, signal }));
}, enabled: id !== '' });

/** Returns a file only after the whole stream finished; a length mismatch or server failure discards every partial byte. */
export async function downloadTrail(scope: TraceScope, filters: TraceSearch, format: AuditExportFormat, signal?: AbortSignal): Promise<Blob> {
  const query = new URLSearchParams({ ...wireFilters(filters), format });
  const base = scope.type === 'system' ? '/admin/audit/export' : `/nodes/${scope.id}/audit/export`;
  const response = await traceRequest(`${base}?${query}`, { ...(signal === undefined ? {} : { signal }) });
  const blob = await response.blob();
  const expected = response.headers.get('Content-Length');
  if (expected !== null && Number(expected) !== blob.size) throw new Error('the download was incomplete');
  if (response.headers.get('X-Orch-Incomplete') === 'true') throw new Error('the download was incomplete');
  return blob;
}

/** Removed environments of a home tree, filtered by each environment's current inspection guard. */
export const keptEnvironmentsQuery = (node: string) => infiniteQueryOptions({ queryKey: ['trace-deleted', 'environments', node], initialPageParam: null as string | null, queryFn: ({ pageParam, signal }): Promise<{ items: DeletedEnvironment[]; next_cursor: string | null }> => ok(api.GET('/nodes/{id}/environments/deleted', { params: { path: { id: node }, query: { under: true, limit: TRAIL_PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal })), getNextPageParam: (page) => page.next_cursor, staleTime: 0, gcTime: 0 });
export const keptDocumentsQuery = (node: string) => infiniteQueryOptions({ queryKey: ['trace-deleted', 'documents', node], initialPageParam: null as string | null, queryFn: ({ pageParam, signal }) => traceJson<{ items: KeptDocumentRef[]; next_cursor: string | null }>(`/nodes/${node}/documents?deleted=only&under=true&limit=${TRAIL_PAGE_SIZE}${pageParam === null ? '' : `&cursor=${pageParam}`}`, { signal }), getNextPageParam: (page) => page.next_cursor, staleTime: 0, gcTime: 0 });
export const retainedEnvironmentQuery = (id: string) => queryOptions({ queryKey: ['deleted-inspection', 'environment', id], queryFn: ({ signal }): Promise<RetainedEnvironment> => ok(api.GET('/environments/{id}', { params: { path: { id }, query: { inspect_deleted: true } }, signal })), staleTime: 0, gcTime: 0 });
/** The removal revision is independent of ordinary settings; it is captured by the current review. */
export function restoreEnvironment(id: string, revision: number, key: string): Promise<Job> { return traceJson(`/environments/${id}/restore`, { method: 'POST', body: { expected_revision: revision }, key }); }
/** An audit event can restore only its matching deletion, never the object's newer group. */
export function restoreTraceObject(kind: string, id: string, deletionId: string, key: string): Promise<Node | Document> {
  return traceJson(`/${kind === 'document' ? 'documents' : 'nodes'}/${id}/restore`, { method: 'POST', body: { expected_deletion_id: deletionId }, key });
}
export function purgeEnvironment(id: string, name: string, revision: number, key: string): Promise<Job> { return ok(api.DELETE('/environments/{id}/purge', { params: { path: { id }, query: { confirm: name, expected_revision: revision } }, ...repeatable(key) })); }
export function scheduleEnvironmentRemoval(id: string, name: string, key: string): Promise<RetainedEnvironment> { return ok(api.DELETE('/environments/{id}', { params: { path: { id }, query: { confirm: name } }, ...repeatable(key) })); }
export const retentionJobQuery = jobQuery;
