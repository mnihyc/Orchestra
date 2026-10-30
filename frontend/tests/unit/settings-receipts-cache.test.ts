/** Safe secret metadata and session acknowledgements update only their actual account/settings owner. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { SecretInfo } from '@/generated/api-types';

const cacheOf = () => new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
const info: SecretInfo = { name: 'DB_PASSWORD', node: { id: 'place', kind: 'org', name: 'Acme', redacted: false }, rotated_at: '2026-10-24T00:00:00Z' };

for (const owner of ['nodes', 'environments']) it(`applies only returned metadata to the ${owner} secret list without reading it again`, async () => {
  const cache = cacheOf();
  const key = ['secrets', 'place'];
  const accepted = { ...info, node: owner === 'nodes' ? info.node : null };
  cache.setQueryData(key, []);
  cache.setQueryData(['secrets', 'another-place'], []);
  const read = vi.fn(() => Promise.resolve([]));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('PUT', `/${owner}/place/secrets/DB_PASSWORD`, { input: { value: 'a fixture-only value' }, answer: accepted })!.changes, cache);
    expect(cache.getQueryData(key)).toEqual([accepted]);
    expect(cache.getQueryData(['secrets', 'another-place'])).toEqual([]);
    expect(JSON.stringify(cache.getQueryData(key))).not.toContain('a fixture-only value');
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); cache.clear(); }
});

for (const shown of [true, false]) it(`retakes a ${shown ? 'shown' : 'hidden'} secret list after deletion, letting the server reveal an inherited name`, async () => {
  const cache = cacheOf();
  const key = ['secrets', 'place'];
  const parent = { ...info, node: { ...info.node!, id: 'parent' } };
  cache.setQueryData(['settings', 'place'], []);
  const settingsRead = vi.fn(() => Promise.resolve([]));
  const stopSettings = new QueryObserver(cache, { queryKey: ['settings', 'place'], queryFn: settingsRead }).subscribe(() => undefined);
  cache.setQueryData(key, [info]);
  const read = vi.fn(() => Promise.resolve([parent]));
  const stop = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
  try {
    await applyChanges(findAct('DELETE', '/nodes/place/secrets/DB_PASSWORD')!.changes, cache);
    expect(cache.getQueryData(key)).toEqual(shown ? [parent] : undefined);
    expect(read).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(settingsRead).not.toHaveBeenCalled();
  } finally { stop(); stopSettings(); cache.clear(); }
});

it('retires an older secret list read before applying accepted rotation metadata', async () => {
  const cache = cacheOf();
  const key = ['secrets', 'place'];
  let finish!: (rows: SecretInfo[]) => void;
  const old = new Promise<SecretInfo[]>((resolve) => { finish = resolve; });
  let signal: AbortSignal | undefined;
  cache.setQueryData(key, [info]);
  const read = ({ signal: given }: { signal: AbortSignal }) => { signal = given; return old; };
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: 0 }).subscribe(() => undefined);
  const accepted = { ...info, rotated_at: '2026-10-24T00:01:00Z' };
  try {
    await applyChanges(findAct('PUT', '/nodes/place/secrets/DB_PASSWORD', { answer: accepted })!.changes, cache);
    expect(signal?.aborted).toBe(true);
    expect(cache.getQueryData(key)).toEqual([accepted]);
    finish([info]); await old;
    expect(cache.getQueryData(key)).toEqual([accepted]);
  } finally { stop(); cache.clear(); }
});

it('removes the acknowledged other session and keeps the current browser without another read', async () => {
  const cache = cacheOf();
  const current = { id: 'this-browser', current: true, user_agent: 'Chrome' };
  cache.setQueryData(['sessions'], [current, { id: 'other-browser', current: false, user_agent: 'Safari' }]);
  const read = vi.fn(() => Promise.resolve([]));
  const stop = new QueryObserver(cache, { queryKey: ['sessions'], queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('DELETE', '/me/sessions/other-browser')!.changes, cache);
    expect(cache.getQueryData(['sessions'])).toEqual([current]);
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); cache.clear(); }
});
