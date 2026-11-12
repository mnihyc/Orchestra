/** Current admin/custody readers use their declared generated operations and the one API client. */
import { infiniteQueryOptions, queryOptions, type QueryClient } from '@tanstack/react-query';
import { api, done, ok } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { PAGE_SIZE } from '@/api/nodes';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { ApiTokenActivityPage, ApiTokenActivityQuery } from '@/generated/api-types';
import type { AdminSession, AdminToken, ReachPage, ReachSnapshot, UploadInventoryItem, ReachExport } from './adminReachTypes';

function current(reader: ReturnType<typeof libraryReader>, signal: AbortSignal): void {
  signal.throwIfAborted();
  if (!sameLibraryReader(reader)) throw new DOMException('The requesting reader changed', 'AbortError');
}
const pageQuery = (cursor: string | null) => ({ limit: PAGE_SIZE, ...(cursor === null ? {} : { cursor }) });
/** Every page retains its native opaque cursor; no total is inferred from a bounded answer. */
function pages<Row>(key: readonly unknown[], admin: boolean, read: (cursor: string | null, signal: AbortSignal) => Promise<ReachPage<Row>>) {
  const reader = libraryReader();
  return infiniteQueryOptions({
    ...(admin ? ADMIN_QUERY : {}), queryKey: [...key, admin, reader], initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<ReachPage<Row>> => {
      current(reader, signal); const answer = await read(pageParam, signal); current(reader, signal); return answer;
    },
    getNextPageParam: (page) => page.next_cursor, retry: false, staleTime: 0, gcTime: 0,
  });
}
export const reachKeys = {
  tokens: ['admin-credentials', 'tokens'] as const,
  sessions: (person: string) => ['admin-credentials', 'sessions', person] as const,
  uploads: ['own-uploads'] as const,
  snapshots: (environment: string) => ['environment-snapshots', environment] as const,
};
export function adminTokensQuery(person?: string) {
  return pages<AdminToken>([...reachKeys.tokens, person ?? null], true, (cursor, signal) => person === undefined
    ? ok(api.GET('/admin/tokens', { params: { query: { ...pageQuery(cursor), cursor, owner_id: null } }, signal }))
    : ok(api.GET('/admin/users/{id}/tokens', { params: { path: { id: person }, query: { ...pageQuery(cursor), cursor, owner_id: null } }, signal })));
}
export function adminSessionsQuery(person: string) {
  return pages<AdminSession>(reachKeys.sessions(person), true, (cursor, signal) => ok(api.GET('/admin/users/{id}/sessions', { params: { path: { id: person }, query: pageQuery(cursor) }, signal })));
}
export function revokeAdminToken(id: string, person?: string): Promise<void> {
  return person === undefined ? done(api.DELETE('/admin/tokens/{id}', { params: { path: { id } } }))
    : done(api.DELETE('/admin/users/{id}/tokens/{token_id}', { params: { path: { id: person, token_id: id } } }));
}
export function revokeAdminSession(person: string, session?: string): Promise<void> {
  return session === undefined ? done(api.DELETE('/admin/users/{id}/sessions', { params: { path: { id: person } } }))
    : done(api.DELETE('/admin/users/{id}/sessions/{session_id}', { params: { path: { id: person, session_id: session } } }));
}
export function adminTokenActivityQuery(person: string, token: string, query: ApiTokenActivityQuery) {
  const reader = libraryReader();
  return queryOptions<ApiTokenActivityPage>({ ...ADMIN_QUERY, queryKey: ['admin-credentials', 'activity', person, token, query, reader] as const,
    queryFn: async ({ signal }): Promise<ApiTokenActivityPage> => {
      current(reader, signal);
      const answer = await ok(api.GET('/admin/users/{id}/tokens/{token_id}/activity', { params: { path: { id: person, token_id: token }, query }, signal }));
      current(reader, signal); return answer;
    }, retry: false, staleTime: 0, gcTime: 0,
  });
}
export const ownUploadsQuery = () => pages<UploadInventoryItem>(reachKeys.uploads, false, (cursor, signal) => ok(api.GET('/me/uploads', { params: { query: pageQuery(cursor) }, signal })));
/** The API content endpoint is distinct from the administrative page routes. */
export function uploadInventoryContentPath(row: UploadInventoryItem, name: string, admin: boolean): string | null {
  if (row.state !== 'complete') return null;
  const query = new URLSearchParams({ name });
  return admin ? row.can_open && row.content_id !== null ? `/admin/uploads/${encodeURIComponent(row.content_id)}/content?${query}` : null
    : row.blob !== null ? `/uploads/blobs/${encodeURIComponent(row.blob)}?${query}` : null;
}
export const environmentSnapshotsQuery = (id: string, admin = false) => pages<ReachSnapshot>(reachKeys.snapshots(id), admin,
  (cursor, signal) => ok(api.GET('/environments/{id}/snapshots', { params: { path: { id }, query: pageQuery(cursor) }, signal })));
export function exportManifestQuery(id: string, admin = false) {
  const reader = libraryReader();
  return queryOptions({ ...(admin ? ADMIN_QUERY : {}), queryKey: ['admin-artifact', 'export', id, admin, reader] as const,
    queryFn: async ({ signal }): Promise<ReachExport> => {
      current(reader, signal); const answer = await ok(api.GET('/exports/{id}', { params: { path: { id } }, signal })); current(reader, signal); return answer;
    }, retry: false, staleTime: 0, gcTime: 0,
  });
}

/** Release/cancel only the target the personal inventory actually supplied. */
export function deleteOwnUpload(row: UploadInventoryItem): Promise<void> {
  if (!row.can_delete) return Promise.reject(new Error('This upload has no current delete action'));
  if (row.kind === 'session' && row.session_id !== null) return done(api.DELETE('/uploads/sessions/{upload_id}', { params: { path: { upload_id: row.session_id } } }));
  if (row.kind === 'completed' && row.blob !== null) return done(api.DELETE('/uploads/blobs/{hash}', { params: { path: { hash: row.blob } } }));
  return Promise.reject(new Error('This upload has no published cancellation target'));
}

export function artifactSnapshotQuery(id: string, admin = false) {
  const reader = libraryReader();
  return queryOptions({ ...(admin ? ADMIN_QUERY : {}), queryKey: ['admin-artifact', 'snapshot', id, admin, reader] as const,
    queryFn: async ({ signal }): Promise<ReachSnapshot> => {
      current(reader, signal); const answer = await ok(api.GET('/snapshots/{id}', { params: { path: { id } }, signal })); current(reader, signal); return answer;
    }, retry: false, staleTime: 0, gcTime: 0,
  });
}

/** The source cache accepts exact supplier keys; a prefix itself is never a read request. */
export async function refreshReach(cache: QueryClient, prefix: readonly unknown[]): Promise<void> {
  const keys = cache.getQueryCache().findAll({ queryKey: prefix }).map((query) => query.queryKey);
  await Promise.all(keys.map((queryKey) => cache.invalidateQueries({ queryKey, exact: true })));
}
