// @vitest-environment node
/** A revoke acknowledgement repairs its exact native history owner, without inventing a row. */
import { setImmediate as nextTurn } from 'node:timers/promises';
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { Invitation } from '@/generated/api-types';
import { me, userRef } from '../doubles/fixtures';

const owner = userRef(me(1, 'owner', 'Owner'));
function invitation(id: string, orgId: string, overrides: Partial<Invitation> = {}): Invitation {
  return {
    id, kind: 'org', org: { id: orgId, kind: 'org', name: orgId, redacted: false },
    node: null, chat: null, role: 'viewer', permissions: [], form: 'link', email: null,
    max_uses: 2, uses: 0, note: '', expires_at: '2026-10-24T00:00:00Z', revoked: false,
    created_by: owner, created_at: '2026-10-23T00:00:00Z', ...overrides,
  };
}
const offered = invitation('offer-to-revoke', 'its-org');
const historical = invitation('earlier-offer', 'its-org', { revoked: true });
// A use can commit before revocation; only the native replacement knows its complete history.
const revoked = { ...offered, uses: 1, revoked: true };
const revokeChanges = (id = offered.id) => findAct('DELETE', `/invitations/${id}`)?.changes ?? [];

function observe(cache: QueryClient, orgId: string, rows: Invitation[], active = true) {
  const key = ['invitations', orgId];
  const read = vi.fn((_context: { signal: AbortSignal }) => Promise.resolve(rows));
  cache.setQueryData(key, rows);
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const stop = active ? observer.subscribe(() => undefined) : () => observer.destroy();
  return { key, read, observer, stop };
}

it('reads only the visible list containing the revoked invitation and retains its native history row', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // Insert an unrelated visible list first: the first list of a kind is not the act's owner.
  const unrelated = observe(cache, 'other-org', [invitation('other-offer', 'other-org')]);
  const held = observe(cache, 'its-org', [offered, historical]);
  held.read.mockResolvedValue([revoked, historical]);
  try {
    await applyChanges(revokeChanges(), cache);
    expect(held.read).toHaveBeenCalledOnce();
    expect(unrelated.read).not.toHaveBeenCalled();
    expect(cache.getQueryData(held.key)).toEqual([revoked, historical]);
    expect(cache.getQueryData<Invitation[]>(held.key)).toHaveLength(2);
    expect(cache.getQueryData(unrelated.key)).toEqual([invitation('other-offer', 'other-org')]);
  } finally { held.stop(); unrelated.stop(); cache.clear(); }
});

it('expires the hidden owning list without reading it or an unrelated visible org', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const unrelated = observe(cache, 'other-org', [invitation('other-offer', 'other-org')]);
  const hidden = observe(cache, 'its-org', [offered, historical], false);
  try {
    await applyChanges(revokeChanges(), cache);
    expect(cache.getQueryData(hidden.key)).toBeUndefined();
    expect(hidden.read).not.toHaveBeenCalled();
    expect(unrelated.read).not.toHaveBeenCalled();
  } finally { hidden.stop(); unrelated.stop(); cache.clear(); }
});

it('cancels an old owning-list read and rejects its late pre-revocation answer', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const held = observe(cache, 'its-org', [offered, historical]);
  let oldDone!: (value: Invitation[]) => void;
  let aborted = false;
  const old = new Promise<Invitation[]>((resolve) => { oldDone = resolve; });
  held.read.mockImplementationOnce(({ signal }) => {
    signal.addEventListener('abort', () => { aborted = true; });
    return old;
  }).mockResolvedValue([revoked, historical]);
  const earlier = cache.refetchQueries({ queryKey: held.key, exact: true });
  try {
    await nextTurn();
    expect(held.read).toHaveBeenCalledOnce();
    await applyChanges(revokeChanges(), cache);
    expect(aborted).toBe(true);
    expect(held.read).toHaveBeenCalledTimes(2);
    expect(cache.getQueryData(held.key)).toEqual([revoked, historical]);
    oldDone([offered, historical]); await earlier; await nextTurn();
    expect(cache.getQueryData(held.key)).toEqual([revoked, historical]);
  } finally { oldDone([offered, historical]); await earlier; held.stop(); cache.clear(); }
});

it('does not refresh a replacement query when the old owner retires during cancellation', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const held = observe(cache, 'its-org', [offered, historical]);
  const cancel = cache.cancelQueries.bind(cache);
  let replacement: ReturnType<typeof observe> | undefined;
  const cancelling = vi.spyOn(cache, 'cancelQueries').mockImplementationOnce(async (filters) => {
    await cancel(filters);
    cache.removeQueries({ queryKey: held.key, exact: true });
    replacement = observe(cache, 'its-org', [revoked, historical]);
  });
  try {
    await applyChanges(revokeChanges(), cache);
    expect(cancelling).toHaveBeenCalledOnce();
    expect(replacement?.read).not.toHaveBeenCalled();
    expect(held.read).not.toHaveBeenCalled();
    expect(cache.getQueryData(held.key)).toEqual([revoked, historical]);
  } finally { cancelling.mockRestore(); replacement?.stop(); held.stop(); cache.clear(); }
});

it('does not guess an invitation list when the acknowledged invitation is not held anywhere', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const shown = observe(cache, 'other-org', [invitation('other-offer', 'other-org')]);
  try {
    await applyChanges(revokeChanges('unheld-offer'), cache);
    expect(shown.read).not.toHaveBeenCalled();
    expect(cache.getQueryData(shown.key)).toEqual([invitation('other-offer', 'other-org')]);
  } finally { shown.stop(); cache.clear(); }
});
