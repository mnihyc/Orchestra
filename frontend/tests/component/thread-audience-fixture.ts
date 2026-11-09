/** Native-shaped audience replies for conversation fixtures, without replacing the banner. */
import type { PermissionPage, Thread, UserRef } from '@/generated/api-types';
import { CATALOGUE, named, pageFor } from '../unit/general-access-fixtures';

export function audienceReply(request: Request, thread: Pick<Thread, 'id' | 'name' | 'owner'>, viewers: readonly UserRef[] = []): Response | null {
  if (request.method !== 'GET') return null;
  const path = new URL(request.url).pathname;
  if (path === '/api/v1/permissions') return Response.json(CATALOGUE.reply);
  if (path === `/api/v1/nodes/${thread.id}/settings`) return Response.json([{
    key: 'orch_tools.owner_reads', value: false, own: null, writable: false, locked_by: null,
    source: { scope: 'default', node: null }, parts: [],
  }]);
  if (path !== `/api/v1/nodes/${thread.id}/permissions`) return null;
  const base = pageFor('thread');
  const page: PermissionPage = { ...base, object: { kind: 'thread', id: thread.id, name: thread.name }, owner: thread.owner,
    owners: [named({ kind: 'user', user: thread.owner }, 1, { created_by: thread.owner })], state: 'custom',
    rows: base.rows.map((row) => ({ ...row, inherit: false, inherited: null, locked: null,
      entries: row.permission === 'thread.view' ? viewers.map((user, index) => named({ kind: 'user', user }, index + 2, { created_by: thread.owner })) : [] })) };
  return Response.json(page);
}
