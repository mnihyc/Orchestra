/** Exact request presentation and deletion identities preserve responsibility, groups and independent restore results. */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { traceHref, traceSearch } from '@/features/trace/params';
import { recordedHolder, requestGroups } from '@/features/trace/requests';
import { bulkRestorable, planGroups, restoreBlock, runRestores, selectionKey, type RestoreResult, type RestoreReview } from '@/features/trace/restore';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';

const api = vi.hoisted(() => ({ restoreEnvironment: vi.fn(), restoreTraceObject: vi.fn(), traceJson: vi.fn() }));
vi.mock('@/api/trace', () => api);
function entry(id: string, kind = 'thread', group = 'first-deletion'): TraceEntry {
  return { id, at: '2026-10-12T12:00:00Z', actor: { kind: 'system' }, action: 'node.deleted', kind: 'deleted', event_deletion_id: group, request_id: null, object: { deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: false, can_inspect: false, id, kind, name: id, state: 'deleted', deletion_id: group, can_restore: true }, node: null, details: null, history: true, as_admin: false, override: false, summary: 'The same words', ip: null };
}
const reviewed = (given: TraceEntry, ancestors: string[] = []): RestoreReview => ({ entry: given, actKey: `act-${given.id}`, ancestors: ancestors.map((id) => ({ id, kind: 'project', name: id, redacted: false })) });
beforeEach(() => { vi.clearAllMocks(); });
describe('one request and its exact records', () => {
  it('extends one request across nonadjacent pages while keeping identical separate requests and unknown legacy rows separate', () => {
    const a = { ...entry('a'), request_id: 'one' }; const b = { ...entry('b'), request_id: 'two' }; const c = { ...entry('c'), request_id: 'one' }; const legacy = entry('legacy');
    const groups = requestGroups([a, b, legacy, c, a], 'day', 'UTC', true);
    expect(groups[0]?.lines.map((line) => line.rows.map((row) => row.id))).toEqual([['a', 'c'], ['b'], ['legacy']]);
    expect(groups[0]?.lines[0]).toMatchObject({ chosen: a, mayContinue: true });
    expect(requestGroups([a, c], 'kind', 'UTC', false)[0]?.lines[0]?.mayContinue).toBe(false);
  });
  it('uses the recorded holder name and preserves direct-only or included-agent URL choices', () => {
    expect(recordedHolder({ holder: { id: 'now-renamed', name: 'Earlier group name' } })).toBe('Earlier group name');
    expect(recordedHolder({ holder: { id: 'unknown' } })).toBeNull();
    expect(traceSearch({ actor: 'person', include_agents: 'false' })).toEqual({ actor: 'person', include_agents: false });
    expect(traceHref({ type: 'system', name: 'System' }, 'person')).toBe('/admin/audit?actor=person&include_agents=true');
    expect(traceHref({ type: 'system', name: 'System' }, 'person', { include_agents: false })).toBe('/admin/audit?include_agents=false&actor=person');
  });
});
describe('the current deletion, never just its time', () => {
  it('two deletions at one held time have different identities; an old or unknown event has no tick', () => {
    const old = entry('same-object'); const newer = { ...old, object: { ...old.object, deletion_id: 'second-deletion' } };
    expect(old.at).toBe(newer.at); expect(restoreBlock(old, newer)).toBe('stale'); expect(bulkRestorable(newer)).toBe(false);
    expect(bulkRestorable({ ...old, event_deletion_id: null })).toBe(false); expect(bulkRestorable({ ...old, object: { ...old.object, can_restore: false } })).toBe(false);
    expect(restoreBlock(old, { ...old, object: { ...old.object, state: 'purged' } })).toBe('purged');
    expect(bulkRestorable(old)).toBe(true);
  });
  it('deduplicates an actual selected parent group, preserving older child deletions and independent environments', () => {
    const rows = planGroups([reviewed(entry('parent')), reviewed(entry('child', 'topic'), ['parent']), reviewed(entry('doc', 'document'), ['parent', 'child']), reviewed(entry('old-child', 'topic', 'earlier'), ['parent']), { ...reviewed(entry('env', 'environment'), ['parent']), revision: 17 }]);
    expect(rows.map((row) => row.includedIn ?? null)).toEqual([null, selectionKey(entry('parent')), selectionKey(entry('parent')), null, null]);
  });
  it('keeps a success when another item fails, carries each guard and uses the same act keys on a repeat attempt', async () => {
    const node = reviewed(entry('node')); const doc = reviewed(entry('doc', 'document')); const env = { ...reviewed(entry('env', 'environment')), revision: 17 };
    api.restoreTraceObject.mockImplementation((kind: string) => { if (kind === 'document') return Promise.reject(new Error('refused')); return Promise.resolve({ id: 'node', archived: true }); });
    api.restoreEnvironment.mockResolvedValue({ id: 'actual-job', state: 'queued' });
    const results = new Map<string, RestoreResult>(); await runRestores([node, doc, env], (key, result) => results.set(key, result));
    expect(results.get(selectionKey(node.entry))?.state).toBe('restored'); expect(results.get(selectionKey(doc.entry))?.state).toBe('failed'); expect(results.get(selectionKey(env.entry))).toMatchObject({ state: 'job', value: { id: 'actual-job' } });
    await runRestores([doc], () => undefined);
    expect(api.restoreTraceObject.mock.calls.filter(([kind]) => kind === 'document')).toEqual([['document', 'doc', 'first-deletion', 'act-doc'], ['document', 'doc', 'first-deletion', 'act-doc']]);
    expect(api.restoreEnvironment).toHaveBeenCalledWith('env', 17, 'act-env');
  });
});
