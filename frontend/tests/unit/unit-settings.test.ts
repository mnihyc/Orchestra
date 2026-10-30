/** Editing one unit key preserves unrelated confirmed values and refuses a stale own revision before writing. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { clearSetting, putSetting } from '@/api/settings';
import type { ResolvedSetting } from '@/generated/api-types';

interface Reply { data: unknown; response: Response }
interface Write { body: { expected_revision: number; config?: Record<string, unknown>; runtime_config?: Record<string, unknown> } }
const server = vi.hoisted(() => {
  const own: Record<string, unknown> = {};
  return { own, key: '', ownRevision: 8,
    get: vi.fn<(path: string) => Promise<Reply>>(), patch: vi.fn<(path: string, write: Write) => Promise<Reply>>(),
  };
});
vi.mock('@/api/client', async (original) => ({ ...await original<object>(), api: { GET: server.get, PATCH: server.patch } }));

function current(): ResolvedSetting {
  const present = Object.hasOwn(server.own, server.key);
  return { key: server.key, value: present ? server.own[server.key] : {}, own: present ? { value: server.own[server.key], revision: server.ownRevision, locked: false } : null,
    source: { scope: 'default', node: null }, parts: [], locked_by: null, writable: true };
}

function reply(path: string, rows: ResolvedSetting[] = [current()]): Reply {
  const unit = { config: server.own, runtime_config: server.own, revision: 12 };
  return { data: path.endsWith('/settings') ? rows : path.endsWith('/sync') ? { environment: unit, seq: 0 } : unit,
    response: new Response(null, { status: 200 }) };
}

beforeEach(() => {
  vi.clearAllMocks(); queryClient.clear(); server.ownRevision = 8;
  server.get.mockImplementation((path) => Promise.resolve(reply(path)));
  server.patch.mockImplementation((_, write) => {
    server.own = write.body.config ?? write.body.runtime_config ?? {}; server.ownRevision += 1;
    return Promise.resolve({ data: {}, response: new Response(null, { status: 200 }) });
  });
});
afterEach(() => { queryClient.clear(); });

it.each(['environment', 'provider'] as const)('a %s command edit keeps variables, secret names and idle policy without adding secret values', async (kind) => {
  const prefix = kind === 'environment' ? 'env' : 'runtime';
  server.key = `${prefix}.after_up`;
  const before = { [`${prefix}.variables`]: { KEEP: 'present' }, [`${prefix}.secret_names`]: ['API_TOKEN'], [`${prefix}.idle_stop_minutes`]: 10, [server.key]: {} };
  server.own = { ...before };
  const commands = { inherited: null, tools: { command: ['/opt/setup', 'a path', '$HOME/bin'], timeout_secs: 45 } };
  const returned = await putSetting({ kind, id: 'unit' }, server.key, { value: commands, expectedRevision: 8 });
  expect(server.get).toHaveBeenCalledWith(kind === 'environment' ? '/environments/{id}/sync' : '/providers/{id}', expect.objectContaining({ params: { path: { id: 'unit' } } }));
  expect(server.patch).toHaveBeenCalledTimes(1);
  expect(server.patch).toHaveBeenCalledWith(kind === 'environment' ? '/environments/{id}' : '/providers/{id}', { params: { path: { id: 'unit' } }, body: {
    expected_revision: 12, [kind === 'environment' ? 'config' : 'runtime_config']: { ...before, [server.key]: commands },
  } });
  expect(returned.own?.revision).toBe(9);
  expect(returned.own?.value).toEqual(commands);
});

it('a newer command revision refuses the draft without sending a resource patch', async () => {
  server.key = 'runtime.after_up'; server.own = { [server.key]: {} }; server.ownRevision = 9;
  await expect(putSetting({ kind: 'provider', id: 'unit' }, server.key, { value: { mine: { command: ['mine'] } }, expectedRevision: 8 })).rejects.toMatchObject({ code: 'conflict.stale_revision' });
  expect(server.patch).not.toHaveBeenCalled();
});

it('an inconsistent own read is refused instead of overwriting confirmed resource configuration', async () => {
  server.key = 'env.after_up'; server.own = { [server.key]: { committed: { command: ['committed'] } } };
  const row = current(); row.own = { value: { older: { command: ['older'] } }, revision: 8, locked: false };
  server.get.mockImplementation((path) => Promise.resolve(reply(path, [row])));
  await expect(putSetting({ kind: 'environment', id: 'unit' }, server.key, { value: {}, expectedRevision: 8 })).rejects.toMatchObject({ code: 'conflict.stale_revision' });
  expect(server.patch).not.toHaveBeenCalled();
});

it('resetting the local command layer keeps every other resource setting', async () => {
  server.key = 'env.after_up'; server.own = { [server.key]: { mine: { command: ['mine'] } }, 'env.variables': { KEEP: 'present' } };
  const returned = await clearSetting({ kind: 'environment', id: 'unit' }, server.key);
  expect(server.own).toEqual({ 'env.variables': { KEEP: 'present' } });
  expect(returned.own).toBeNull();
});
