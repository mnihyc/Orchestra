/** Current object guards, retained names and removal revisions stay separate from audit authority. */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { objectActs } from '@/features/trace/objects';

const { request, deleteRequest } = vi.hoisted(() => ({ request: vi.fn(), deleteRequest: vi.fn() }));
vi.mock('@/api/client', () => ({ api: { request, DELETE: deleteRequest }, ok: async (given: Promise<{ response: Response; data?: unknown }>) => { const reply = await given; if (!reply.response.ok) throw new Error('refused'); return reply.data; }, repeatable: (key: string) => ({ headers: { 'Idempotency-Key': key } }) }));
import { downloadTrail, purgeEnvironment, restoreEnvironment } from '@/api/trace';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';

beforeEach(() => { request.mockReset(); deleteRequest.mockReset(); });

const object: TraceEntry['object'] = { deletion_id: null, deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, id: 'retained-id', kind: 'thread', name: 'Last name', state: 'deleted', can_open: true, can_inspect: false, can_restore: false };
describe('a permanent row and content authority', () => {
  it('keeps a deleted name without borrowing its live-view right for inspection or restoration', () => { expect(objectActs(object)).toEqual({ open: null, inspect: null, restore: false }); expect(objectActs({ ...object, can_inspect: true })).toEqual({ open: null, inspect: '/inspect/thread/retained-id', restore: false }); });
  it('a purged thing has no content acts even when a stale response still says yes', () => { expect(objectActs({ ...object, state: 'purged', can_inspect: true, can_restore: true })).toEqual({ open: null, inspect: null, restore: false }); });
  it('a live thing opens only under its own current view guard', () => { expect(objectActs({ ...object, state: 'live', can_open: false }).open).toBeNull(); expect(objectActs({ ...object, state: 'live' }).open).toBe('/thread/retained-id'); });
});
describe('requests on kept files', () => {
  it('restoration sends the removal revision and returns the accepted queued job without declaring files restored', async () => {
    const job = { id: 'server-job', kind: 'environment_restore', state: 'queued' };
    request.mockResolvedValueOnce({ response: new Response(JSON.stringify(job), { status: 202 }) });
    expect(await restoreEnvironment('env-id', 17, 'one-act')).toEqual(job);
    expect(request).toHaveBeenLastCalledWith('post', '/environments/env-id/restore', { parseAs: 'stream', body: { expected_revision: 17 }, headers: { 'Idempotency-Key': 'one-act' } });
  });
  it('permanent removal names the reviewed thing and its deletion revision', async () => {
    const job = { id: 'purge-job', kind: 'environment_purge', state: 'queued' };
    deleteRequest.mockResolvedValueOnce({ response: new Response(JSON.stringify(job), { status: 202 }), data: job });
    expect(await purgeEnvironment('env-id', 'Kept workspace', 19, 'purge-act')).toEqual(job);
    expect(deleteRequest).toHaveBeenCalledOnce();
    expect(deleteRequest).toHaveBeenLastCalledWith('/environments/{id}/purge', {
      params: { path: { id: 'env-id' }, query: { confirm: 'Kept workspace', expected_revision: 19 } },
      headers: { 'Idempotency-Key': 'purge-act' },
    });
  });
  it('an incomplete stream yields no downloadable trail', async () => {
    request.mockResolvedValueOnce({ response: new Response('cut off', { headers: { 'Content-Length': '40' } }) });
    await expect(downloadTrail({ type: 'system', name: 'System' }, { actor: 'person', action: 'node.deleted', kind: 'deleted', from: '2026-10-01', to: '2026-10-13' }, 'jsonl')).rejects.toThrow('incomplete');
  });
  it('a server-marked incomplete trail yields no file even without a content length', async () => {
    request.mockResolvedValueOnce({ response: new Response('partial row', { headers: { 'X-Orch-Incomplete': 'true' } }) });
    await expect(downloadTrail({ type: 'system', name: 'System' }, {}, 'jsonl')).rejects.toThrow('incomplete');
  });
  it('a complete trail keeps all of its UTF-8 bytes', async () => {
    const text = '日志\n';
    const bytes = new TextEncoder().encode(text);
    request.mockResolvedValueOnce({ response: new Response(bytes, { headers: { 'Content-Length': String(bytes.byteLength) } }) });
    const file = await downloadTrail({ type: 'system', name: 'System' }, {}, 'jsonl');
    expect(await file.text()).toBe(text);
  });
});
