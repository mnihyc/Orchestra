/** Complete immutable windows, source identity, independent panes and late-answer fencing. */
import { expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import type { DirListing } from '@/generated/api-types';
import { sortEntries } from '@/realtime/stores/directoryTypes';
import { door, entry, page, pending, rig, STAMP } from './directory-window-fixtures';

it('keeps every sibling beyond 5000 with native ordering and whole-page replacement', async () => {
  const r = rig(), view = r.open();
  const names = Array.from({ length: 5007 }, (_, index) => String(index).padStart(5, '0'));
  r.event({ reset: true, changed: names.slice(0, 5000).map((name) => entry(name)), revision: 'r',
    next_cursor: 'after-5000', truncated: true, as_of: STAMP, observed_from: STAMP });
  expect(view.read().entries).toHaveLength(5000);
  r.fetch.mockResolvedValueOnce(page(names.slice(5000), { revision: 'r' }));
  const first = view.read().entries;
  expect(await view.next()).toBe('done');
  expect(r.fetch).toHaveBeenLastCalledWith(door, '', expect.objectContaining({ cursor: 'after-5000', source: 'live', limit: 5000 }));
  expect([...first, ...view.read().entries].map((row) => row.name)).toEqual(names);
  expect(view.read().entries).toHaveLength(7);
  expect(view.read().page?.ordinal).toBe(2);
  expect(await view.previous()).toBe('done');
  expect(view.read().entries).toBe(first); expect(r.fetch).toHaveBeenCalledTimes(1);
  await view.release();
});

it('accepts byte-short pages without offset math, preserves Unicode order and refuses an oversized whole page', async () => {
  const r = rig(), view = r.open();
  const names = ['Z', 'z', 'ä', '\uE000', '\u{10000}'];
  r.fetch.mockResolvedValueOnce(page(names, { next_cursor: 'byte-bound', truncated: true }));
  await view.first();
  expect(view.read().entries.map((row) => row.name)).toEqual(names);
  expect(sortEntries([...view.read().entries].reverse()).map((row) => row.name)).toEqual(names);
  r.fetch.mockResolvedValueOnce(page(['tail'])); await view.next();
  expect(r.fetch.mock.calls[1]?.[2].cursor).toBe('byte-bound');
  expect(view.read().entries.map((row) => row.name)).toEqual(['tail']);
  const before = view.read().entries;
  const huge = page(['x'.repeat(2 * 1024 * 1024)]);
  r.fetch.mockResolvedValueOnce(huge);
  expect(await view.restart()).toBe('capacity');
  expect(view.read().entries).toBe(before); expect(view.read().capacity?.reason).toBe('bytes');
  expect(r.store.usage().bytes).toBeLessThanOrEqual(8 * 1024 * 1024);
  await view.release();
});

it('shares one stable watch but keeps independent pane pages stale on a reset, with explicit restart', async () => {
  const r = rig(), left = r.open('', 'left'), right = r.open('', 'right');
  r.event({ reset: true, changed: [entry('old-head')], revision: 'old', next_cursor: 'old-next', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  r.fetch.mockResolvedValueOnce(page(['old-tail'], { revision: 'old' })); await left.next();
  expect(right.read().entries[0]?.name).toBe('old-head');
  const generation = left.read().generation;
  r.event({ reset: true, changed: [entry('new-head')], revision: 'new', truncated: false });
  expect(left.read()).toMatchObject({ invalidated: true, restartRequired: true, nextCursor: null });
  expect(left.read().entries[0]?.name).toBe('old-tail');
  expect(left.read().generation).not.toBe(generation);
  expect(right.read().entries[0]?.name).toBe('new-head');
  expect(await left.next()).toBe('restart'); expect(r.fetch).toHaveBeenCalledTimes(1);
  r.fetch.mockResolvedValueOnce(page(['fresh-head'], { revision: 'fresh' })); await left.restart();
  expect(left.read().entries[0]?.name).toBe('fresh-head');
  expect(r.subscriptions).toHaveLength(1);
  await left.release(); expect(r.subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  await right.release(); expect(r.subscriptions[0]?.unsubscribe).toHaveBeenCalledTimes(1);
});

it('does not rebase another pane when an HTTP fresh observation changes source or window', async () => {
  const r = rig(), left = r.open('', 'left'), right = r.open('', 'right');
  r.event({ reset: true, changed: [entry('head')], revision: 'old', next_cursor: 'next', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  r.fetch.mockResolvedValueOnce(page(['tail'], { revision: 'old' })); await left.next();
  r.fetch.mockResolvedValueOnce(page(['cached-head'], { source: 'cached', revision: 'cached-window' })); await right.restart();
  expect(left.read().entries[0]?.name).toBe('tail');
  expect(left.read()).toMatchObject({ restartRequired: true, nextCursor: null });
  expect(right.read().source).toBe('cached');
  await left.release(); await right.release();
});

it.each(['revision', 'source', 'as_of', 'observed_from', 'path'] as const)('rejects a continuation with the wrong %s without merging it', async (field) => {
  const r = rig(), view = r.open();
  r.fetch.mockResolvedValueOnce(page(['head'], { next_cursor: 'next', truncated: true })); await view.first();
  const wrong: DirListing = page(['wrong']);
  switch (field) {
    case 'revision': wrong.revision = 'other'; break;
    case 'source': wrong.source = 'cached'; break;
    case 'as_of': wrong.as_of = '2026-01-01T00:00:01Z'; break;
    case 'observed_from': wrong.observed_from = '2025-12-31T23:59:00Z'; break;
    case 'path': wrong.path = 'different'; break;
  }
  r.fetch.mockResolvedValueOnce(wrong);
  expect(await view.next()).toBe('restart');
  expect(view.read().entries[0]?.name).toBe('head');
  expect(view.read()).toMatchObject({ invalidated: true, canNext: false });
  await view.release();
});

it('fences late HTTP and frames on invalidation, refusal, reader retirement and a same-path new binding', async () => {
  const r = rig(), view = r.open();
  const old = pending<DirListing>(); r.fetch.mockReturnValueOnce(old.promise); const reading = view.first();
  r.event({ reset: true, changed: [entry('native')], revision: 'new' });
  old.resolve(page(['late-http'])); await reading;
  expect(view.read().entries[0]?.name).toBe('native');
  const obsolete = r.subscriptions[0];
  r.event({ error: { code: 'authz.denied', detail: 'No longer readable', reason: null, retryable: false } });
  expect(view.read().entries).toEqual([]);
  r.event({ reset: true, changed: [entry('late-frame')] }); expect(view.read().entries).toEqual([]);
  r.store.forget(door.environmentId);
  const replacement = r.open('', 'new', 'new-binding');
  r.event({ reset: true, changed: [entry('replacement')] }, 1);
  obsolete?.onFrame({ op: 'event', id: 1, feed: 'old', data: { reset: true, changed: [entry('old-binding')] } });
  await view.release(); expect(replacement.read().entries[0]?.name).toBe('replacement');
  r.reader(); expect(replacement.read().entries).toEqual([]);
  expect(r.published().size).toBe(0); await replacement.release();
});

it('retains a failed native cause over old later rows and HTTP alone cannot claim recovered Live', async () => {
  const r = rig(), view = r.open();
  r.event({ reset: true, changed: [entry('head')], revision: 'revision', next_cursor: 'next', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  r.fetch.mockResolvedValueOnce(page(['tail'])); await view.next();
  r.event({ error: { code: 'environment.unavailable', detail: 'Watch capacity', reason: 'busy', retryable: true } });
  expect(view.read()).toMatchObject({ status: 'failed', watch: 'failed', watchError: { reason: 'busy', retryable: true }, restartRequired: true });
  expect(view.read().entries[0]?.name).toBe('tail');
  r.store.transport(false); r.store.transport(true);
  expect(view.read().watch).toBe('failed'); expect(r.subscriptions).toHaveLength(1);
  expect(await view.retry()).toBe('restart');
  r.fetch.mockResolvedValueOnce(page(['new'])); await view.restart();
  expect(view.read().watch).toBe('connecting');
  r.event({ reset: true, changed: [entry('new')], revision: 'new' }, 1);
  expect(view.read().watch).toBe('live'); await view.release();
});

it('refuses inaccessible HTTP results on every owned page and retains the normalized API error', async () => {
  const r = rig(), view = r.open(); r.event({ reset: true, changed: [entry('private')] });
  r.fetch.mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: '' }, null));
  await view.restart();
  expect(view.read().entries).toEqual([]); expect(view.read().watch).toBe('refused');
  expect(view.read().error).toBeInstanceOf(ApiError); await view.release();
});

it('follows a long-path byte window shorter than the requested 5000 without omitting a sibling', async () => {
  const r = rig(), parent = Array.from({ length: 14 }, () => 'p'.repeat(240)).join('/'), view = r.open(parent);
  const names = Array.from({ length: 700 }, (_, index) => String(index).padStart(4, '0'));
  const first = page(names.slice(0, 450), { path: parent, next_cursor: 'native-byte-edge', truncated: true });
  const encoded = new TextEncoder().encode(JSON.stringify(first)).length;
  expect(encoded).toBeGreaterThan(1024 * 1024); expect(encoded).toBeLessThan(2 * 1024 * 1024);
  r.fetch.mockResolvedValueOnce(first); await view.first();
  const earlier = view.read().entries;
  r.fetch.mockResolvedValueOnce(page(names.slice(450), { path: parent })); await view.next();
  expect(r.fetch.mock.calls[0]?.[2].limit).toBe(5000);
  expect(r.fetch.mock.calls[1]?.[2].cursor).toBe('native-byte-edge');
  expect([...earlier, ...view.read().entries].map((row) => row.name)).toEqual(names);
  await view.release();
});

it('keeps explicit source reads fenced while auto may resolve to the honest offline cached source', async () => {
  const r = rig(), view = r.open();
  r.fetch.mockResolvedValueOnce(page(['not-the-requested-source'], { source: 'live' }));
  await r.store.load(door, '', { source: 'cached' });
  expect(view.read().entries).toEqual([]); expect(view.read().invalidated).toBe(true);
  r.store.transport(false);
  r.fetch.mockResolvedValueOnce(page(['cached'], { source: 'cached' })); await view.restart();
  expect(view.read()).toMatchObject({ source: 'cached', offline: true, watch: 'offline' });
  expect(view.read().entries[0]?.name).toBe('cached'); await view.release();
});

it('refuses a nonprogressing continuation rather than enabling an endless next-page walk', async () => {
  const r = rig(), view = r.open();
  r.fetch.mockResolvedValueOnce(page(['head'], { next_cursor: 'next', truncated: true })); await view.first();
  r.fetch.mockResolvedValueOnce(page(['bad-tail'], { next_cursor: 'next', truncated: true }));
  expect(await view.next()).toBe('restart');
  expect(view.read().entries[0]?.name).toBe('head'); expect(view.read().nextCursor).toBeNull();
  await view.release();
});

it('applies complete live deltas directly in canonical order without mutating an older observation', async () => {
  const r = rig(), view = r.open(), supplied = entry('z');
  r.event({ reset: true, changed: [supplied], truncated: false });
  const before = view.read().entries; supplied.name = 'outside-mutation';
  expect(view.read().entries[0]?.name).toBe('z');
  r.event({ reset: false, changed: [entry('ä'), entry('Z')], removed: ['z'] });
  expect(view.read().entries.map((row) => row.name)).toEqual(['Z', 'ä']);
  expect(before[0]?.name).toBe('z'); expect(view.read().watch).toBe('live');
  expect(r.fetch).not.toHaveBeenCalled(); await view.release();
});

it.each(['reset', 'invalidated', 'busy', 'refused'] as const)('fences a held later page on %s with no automatic HTTP walk', async (change) => {
  const r = rig(), view = r.open();
  r.event({ reset: true, changed: [entry('head')], revision: 'revision', next_cursor: 'two', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  r.fetch.mockResolvedValueOnce(page(['second'], { next_cursor: 'three', truncated: true })); await view.next();
  const gate = pending<DirListing>(); r.fetch.mockReturnValueOnce(gate.promise); const reading = view.next();
  switch (change) {
    case 'reset': r.event({ reset: true, changed: [entry('new-head')], revision: 'new' }); break;
    case 'invalidated': r.event({ invalidated: true }); break;
    case 'busy': r.event({ error: { code: 'environment.unavailable', detail: 'Busy', reason: 'busy', retryable: true } }); break;
    case 'refused': r.event({ error: { code: 'authz.denied', detail: '', reason: null, retryable: false } }); break;
  }
  gate.resolve(page(['obsolete-third'])); expect(await reading).toBe('released');
  expect(view.read().entries.map((row) => row.name)).toEqual(change === 'refused' ? [] : ['second']);
  expect(view.read().canNext).toBe(false); expect(r.fetch).toHaveBeenCalledTimes(2);
  await view.release();
});

it('authority reload evicts all selected directory pages and cursors while preserving their view owners for fresh native data', async () => {
  const r = rig(), left = r.open('', 'left'), right = r.open('', 'right');
  const ids = [left.id, right.id];
  r.event({ reset: true, changed: [entry('private-head')], revision: 'private', next_cursor: 'two', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  r.fetch.mockResolvedValueOnce(page(['private-tail'], { revision: 'private', next_cursor: 'three', truncated: true }));
  await left.next(); left.expanded(['private-expanded']);
  const gate = pending<DirListing>(); r.fetch.mockReturnValueOnce(gate.promise); const reading = left.next();
  const old = r.subscriptions[0]; if (old === undefined) throw new Error('Expected the old directory subscription');
  const generation = left.read().generation;
  old.onFrame({ op: 'resync', id: 1, feed: old.feed, reload: true });
  for (const view of [left, right]) {
    expect(view.read()).toMatchObject({ entries: [], page: null,
      canNext: false, canPrevious: false, expanded: [], status: 'loading', watch: 'connecting', error: null });
    expect(view.read().nextCursor).toBeUndefined();
  }
  expect([left.id, right.id]).toEqual(ids); expect(left.read().generation).not.toBe(generation);
  expect([...r.published().values()].every((state) => state.entries.length === 0)).toBe(true);
  expect(r.store.usage().rows).toBe(0); expect(r.subscriptions).toHaveLength(2);
  expect(old.unsubscribe).toHaveBeenCalledTimes(1); expect(r.fetch).toHaveBeenCalledTimes(2);
  old.onFrame({ op: 'event', id: 1, feed: old.feed, data: { reset: true, changed: [entry('retired-native')] } });
  gate.resolve(page(['retired-HTTP'], { revision: 'private' })); expect(await reading).toBe('released');
  expect(left.read().entries).toEqual([]); expect(right.read().entries).toEqual([]);
  r.event({ reset: true, changed: [entry('readmitted')], revision: 'current', truncated: false }, 1);
  for (const view of [left, right]) {
    expect(view.read().entries.map((row) => row.name)).toEqual(['readmitted']);
    expect(view.read()).toMatchObject({ status: 'ready', watch: 'live', page: { ordinal: 1 }, restartRequired: false });
  }
  await left.release(); expect(r.subscriptions[1]?.unsubscribe).not.toHaveBeenCalled();
  await right.release(); expect(r.subscriptions[1]?.unsubscribe).toHaveBeenCalledTimes(1);
});

it.each([undefined, false])('ordinary directory gap reload=%s preserves visible stale pages while retiring cursors', async (reload) => {
  const r = rig(), view = r.open();
  r.event({ reset: true, changed: [entry('kept')], revision: 'old', next_cursor: 'old-next', truncated: true });
  const before = view.read().entries, old = r.subscriptions[0];
  if (old === undefined) throw new Error('Expected the old directory subscription');
  old.onFrame({ op: 'resync', id: 1, feed: old.feed, ...(reload === undefined ? {} : { reload }) });
  expect(view.read().entries).toBe(before);
  expect(view.read()).toMatchObject({ invalidated: true, nextCursor: null, canNext: false, canPrevious: false, watch: 'connecting' });
  expect(r.subscriptions).toHaveLength(2); expect(r.fetch).not.toHaveBeenCalled();
  r.event({ reset: true, changed: [entry('fresh')], revision: 'fresh', truncated: false }, 1);
  expect(view.read().entries.map((row) => row.name)).toEqual(['fresh']);
  await view.release();
});

it('authority reload clears a failed sealed watch without reporting a permission refusal or losing its owner', async () => {
  const r = rig(), view = r.open();
  r.event({ reset: true, changed: [entry('private')], revision: 'old' });
  r.event({ error: { code: 'environment.unavailable', detail: 'Capacity', reason: 'busy', retryable: true } });
  expect(view.read().entries.map((row) => row.name)).toEqual(['private']);
  expect(view.read().watch).toBe('failed');
  const old = r.subscriptions[0]; if (old === undefined) throw new Error('Expected the old directory subscription');
  old.onFrame({ op: 'resync', id: 1, feed: old.feed, reload: true });
  expect(view.read()).toMatchObject({ entries: [], status: 'loading', watch: 'connecting', error: null });
  expect(r.subscriptions).toHaveLength(2);
  r.event({ reset: true, changed: [entry('current')], revision: 'current' }, 1);
  expect(view.read().entries.map((row) => row.name)).toEqual(['current']);
  expect(view.read().watch).toBe('live'); await view.release();
});
