// @vitest-environment node
/** A setting act carries its reviewed own revision, including reviewed absence and guarded Undo/reset. */
import { beforeEach, expect, it, vi } from 'vitest';
import type { ResolvedSetting } from '@/generated/api-types';
import { clearSetting, putSetting } from '@/api/settings';

const calls = vi.hoisted(() => ({
  put: vi.fn<(path: string, options: unknown) => Promise<{ data: ResolvedSetting }>>(),
  remove: vi.fn<(path: string, options: unknown) => Promise<{ data: ResolvedSetting }>>(),
}));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(),
  api: { PUT: calls.put, DELETE: calls.remove },
  ok: async (reply: Promise<{ data: ResolvedSetting }>) => (await reply).data,
}));
const row: ResolvedSetting = { key: 'agent.web_search', value: true, own: { value: true, revision: 7, locked: false },
  source: { scope: 'system', node: null }, locked_by: null, writable: true, parts: [] };
beforeEach(() => { vi.clearAllMocks(); calls.put.mockResolvedValue({ data: row }); calls.remove.mockResolvedValue({ data: { ...row, own: null } }); });

it('turns a reviewed absent own value into the native zero guard instead of an unconditional create', async () => {
  await putSetting({ kind: 'node', id: 'topic' }, row.key, { value: true, expectedRevision: null });
  expect(calls.put).toHaveBeenCalledExactlyOnceWith('/nodes/{id}/settings/{key}', {
    params: { path: { id: 'topic', key: row.key } }, body: { value: true, expected_revision: 0 },
  });
});

it('keeps an explicit system save revision and restores its prior lock with the same guard', async () => {
  await putSetting({ kind: 'system' }, row.key, { value: true, locked: false, expectedRevision: 6 });
  expect(calls.put).toHaveBeenCalledExactlyOnceWith('/admin/settings/{key}', {
    params: { path: { key: row.key } }, body: { value: true, locked: false, expected_revision: 6 },
  });
});

it('writes a host own value through its guarded setting endpoint and resets the same revision', async () => {
  const target = {kind:'host',id:'host'} as const;
  await putSetting(target,'cleanup.unused_keep_days',{value:0,expectedRevision:null});
  expect(calls.put).toHaveBeenCalledExactlyOnceWith('/hosts/{id}/settings/{key}', {
    params:{path:{id:'host',key:'cleanup.unused_keep_days'}},body:{value:0,expected_revision:0},
  });
  await clearSetting(target,'cleanup.unused_keep_days',7);
  expect(calls.remove).toHaveBeenCalledExactlyOnceWith('/hosts/{id}/settings/{key}', {
    params:{path:{id:'host',key:'cleanup.unused_keep_days'},query:{expected_revision:7}},
  });
});

it.each([{ kind: 'node', id: 'topic' } as const, { kind: 'system' } as const])('fences removal of the value at $kind', async (target) => {
  await clearSetting(target, row.key, 7);
  expect(calls.remove).toHaveBeenCalledExactlyOnceWith(target.kind === 'node' ? '/nodes/{id}/settings/{key}' : '/admin/settings/{key}', {
    params: { path: target.kind === 'node' ? { id: target.id, key: row.key } : { key: row.key }, query: { expected_revision: 7 } },
  });
});
