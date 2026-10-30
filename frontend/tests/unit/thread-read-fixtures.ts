/** Native-shaped synthetic transport for independent read owners; no live account material. */
import { vi } from 'vitest';
import { coordinateThreadReads, applyThreadFeedRows } from '@/realtime/threadFeed';
import { FeedRegistry } from '@/realtime/sync';
import type { ThreadReadTarget } from '@/realtime/threadScope';
import { A, B, C, answer, opening, projection, snapshot } from './continuation-fixtures';

export { A, B, C };
export const T = snapshot().thread.id;
export const OTHER = '01950000-0000-7000-8000-000000000009';
export const SHARED = '01950000-0000-7000-8000-000000000010';
export const OLDER = '01950000-0000-7000-8000-000000000011';
export function bodyOf(value: ReturnType<typeof answer> | undefined): string {
  return value?.kind === 'agent_message' ? value.payload.text : '';
}
export function ownedRows(target: ThreadReadTarget) {
  return { rows: (data: Parameters<typeof applyThreadFeedRows>[1],reset: boolean) => { applyThreadFeedRows(target,data,reset); } };
}
export function coordinator() {
  const registry = new FeedRegistry();
  const reload = coordinateThreadReads(registry,(_id,ref) => ownedRows(ref));
  return { registry,reload };
}

export function serve(override?: (request: Request) => Promise<Response> | undefined) {
  const calls: Request[] = [];
  const state = { active: A,revision: 1,selectedRevision: 2,seq: 20 };
  vi.stubGlobal('fetch',(input: RequestInfo | URL,init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input,init);
    const url = new URL(request.url);
    calls.push(request);
    const path = /^\/api\/v1\/threads\/([^/]+)(.*)$/.exec(url.pathname);
    if (path?.[1] === undefined || path[2] === undefined) return Promise.reject(new Error('Expected a real native thread URL'));
    const intercepted = override?.(request); if (intercepted !== undefined) return intercepted;
    const native = path[1], suffix = path[2];
    const selected = url.searchParams.get('continuation_id');
    const view = selected ?? state.active;
    const scope = { ...projection(view,state.active,selected === null ? state.revision : state.selectedRevision,selected === null),execution_revision: state.revision };
    const revision = url.searchParams.get('projection_revision');
    if (revision !== null && Number(revision) !== scope.projection_revision) return Promise.resolve(Response.json({
      code: 'conflict.stale_revision',status: 409,title: 'Synthetic stale read',request_id: 'synthetic',detail: 'The selected projection changed' },{ status: 409 }));
    const shared = answer(SHARED,`body-${view}`,A,10);
    if (suffix === '/opening') {
      const cut = snapshot(scope,state.seq);
      return Promise.resolve(Response.json(opening({ ...cut,thread: { ...cut.thread,id: native,active_continuation_id: state.active,execution_revision: state.revision },items: [shared] })));
    }
    if (suffix === '/items') return Promise.resolve(Response.json({ projection: scope,
      items: [{ ...(url.searchParams.get('around') === SHARED ? shared : answer(url.searchParams.get('around') ?? OLDER,`window-${view}`,view,8)),stream: url.searchParams.get('stream') ?? 'main' }],
      turns: [],segments: [],has_older: false,has_newer: true }));
    if (/^\/items\/[^/]+\/output$/.test(suffix)) return Promise.resolve(new Response(`output-${view}`));
    if (/^\/items\/[^/]+$/.test(suffix)) return Promise.resolve(Response.json(shared));
    if (suffix === '/export') return Promise.resolve(new Response(`export-${view}`,{ headers: { 'Content-Disposition': 'attachment; filename="view.md"' } }));
    if (suffix === '/outline') return Promise.resolve(Response.json({ projection: scope,entries: [],seq: state.seq,next_before: null }));
    if (suffix === '/segments') return Promise.resolve(Response.json({ projection: scope,segments: [],seq: state.seq,next_before: null }));
    if (suffix === '/turns') return Promise.resolve(Response.json({ projection: scope,turns: [],segments: [],seq: state.seq,next_before: null }));
    if (suffix === '/raw') return Promise.resolve(Response.json({ projection: scope,events: [],has_older: false }));
    if (suffix === '/inputs' && request.method === 'GET') return Promise.resolve(Response.json({ projection: scope,inputs: [],seq: state.seq,next_after: null,queue_revision: 1 }));
    if (suffix === '/read') return request.clone().json().then((body: unknown) => {
      if (typeof body !== 'object' || body === null || !('seq' in body)) throw new Error('Expected a native read body');
      const selected = 'continuation_id' in body && typeof body.continuation_id === 'string' ? body.continuation_id : null;
      const readScope = { ...projection(selected ?? state.active,state.active,selected === null ? state.revision : state.selectedRevision,selected === null),execution_revision: state.revision };
      if ('projection_revision' in body && body.projection_revision !== readScope.projection_revision) return Response.json({
        code: 'conflict.stale_revision',status: 409,title: 'Synthetic stale read',request_id: 'synthetic',detail: 'The read body names an old projection' },{ status: 409 });
      return Response.json({ projection: readScope,last_read_seq: body.seq });
    });
    return Promise.reject(new Error('Unexpected synthetic native read endpoint'));
  });
  return { calls,state };
}
