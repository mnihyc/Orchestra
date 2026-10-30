/** Shared quotas include heads, selected windows, expansion/selectors and unfinished requests. */
import { expect, it } from 'vitest';
import type { DirListing } from '@/generated/api-types';
import { DIRECTORY_IDLE_MS, DIRECTORY_METADATA_MAX } from '@/realtime/stores/directoryBudget';
import type { DirectoryLease } from '@/realtime/stores/directoryTypes';
import { door, entry, page, pending, rig, STAMP } from './directory-window-fixtures';

it('admits only 24 physical directories, shares a watch and returns capacity instead of polling', async () => {
  const r = rig(), owners = Array.from({ length: 24 }, (_, index) => r.open(String(index)));
  const shared = r.open('0', 'two');
  expect(r.store.usage().admitted).toBe(24); expect(r.subscriptions).toHaveLength(24);
  expect(r.store.open(door, '25', { pane: 'one', binding: 'binding' })).toMatchObject({ ok: false, reason: 'directories' });
  expect(r.fetch).not.toHaveBeenCalled();
  await owners[0]?.release(); expect(r.subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  await shared.release(); expect(r.subscriptions[0]?.unsubscribe).toHaveBeenCalledTimes(1);
  const replacement = r.open('25'); expect(r.store.usage().admitted).toBe(24);
  await Promise.all([...owners.slice(1).map((owner) => owner.release()), replacement.release()]);
});

it('counts all native heads in the shared 20k row budget and never evicts an owned page', async () => {
  const r = rig(), owners = Array.from({ length: 5 }, (_, index) => r.open(String(index)));
  for (let index = 0; index < owners.length; index += 1) r.event({ reset: true,
    changed: Array.from({ length: 5000 }, (_, row) => entry(String(row), String(index))), revision: String(index) }, index);
  expect(r.store.usage().rows).toBe(20_000);
  expect(owners[4]?.read().capacity?.reason).toBe('rows');
  for (const owner of owners.slice(0, 4)) expect(owner.read().entries).toHaveLength(5000);
  expect(r.fetch).not.toHaveBeenCalled();
  await owners[0]?.release();
  r.event({ reset: true, changed: [entry('now-fits', '4')], revision: 'fresh' }, 4);
  expect(owners[4]?.read().entries[0]?.name).toBe('now-fits');
  expect(r.store.usage().rows).toBeLessThanOrEqual(20_000);
  await Promise.all(owners.slice(1).map((owner) => owner.release()));
});

it('charges selector/expansion metadata before adoption and can still record a later refusal', async () => {
  const r = rig(), owner = r.open();
  r.event({ reset: true, changed: [entry('kept')] });
  expect(owner.expanded(['x'.repeat(1300 * 1024)])).toEqual({ ok: true });
  const before = r.store.usage().metadata;
  expect(owner.expanded(['x'.repeat(2 * 1024 * 1024)])).toMatchObject({ ok: false, reason: 'metadata' });
  expect(r.store.usage().metadata).toBe(before);
  r.event({ error: { code: 'environment.unavailable', detail: 'd'.repeat(2 * 1024 * 1024), reason: 'busy', retryable: true } });
  expect(owner.read()).toMatchObject({ status: 'failed', watchError: { code: 'environment.unavailable', retryable: true }, capacity: { reason: 'metadata' } });
  expect(owner.read().entries[0]?.name).toBe('kept');
  expect(r.store.usage().metadata).toBeLessThanOrEqual(DIRECTORY_METADATA_MAX);
  r.event({ error: { code: 'authz.denied', detail: '', reason: null, retryable: false } });
  // Failure seals ordinary late frames, but cannot mask a later authority refusal.
  expect(owner.read().entries).toEqual([]); expect(owner.read().watch).toBe('refused');
  r.reader(); expect(owner.read().entries).toEqual([]); await owner.release();
});

it('bounds all acquisition records without a hidden fallback map', async () => {
  const r = rig(), owners: DirectoryLease[] = [];
  let refused = false;
  for (let index = 0; index < 257; index += 1) {
    const opened = r.store.open(door, '', { pane: String(index), binding: 'binding' });
    if (opened.ok) owners.push(opened.lease); else { expect(['metadata', 'records']).toContain(opened.reason); refused = true; break; }
  }
  expect(refused).toBe(true); expect(r.store.usage().records).toBeLessThanOrEqual(256);
  expect(r.store.usage().metadata).toBeLessThanOrEqual(DIRECTORY_METADATA_MAX);
  expect(r.subscriptions).toHaveLength(1);
  await Promise.all(owners.map((owner) => owner.release()));
});

it('expires unowned state lazily at five minutes and keeps an owned page pinned without idle reads', async () => {
  const r = rig(), owned = r.open('owned'), old = r.open('old');
  r.event({ reset: true, changed: [entry('kept', 'owned')] });
  r.event({ reset: true, changed: [entry('old', 'old')] }, 1); await old.release();
  r.advance(DIRECTORY_IDLE_MS + 1);
  expect(r.fetch).not.toHaveBeenCalled();
  const fresh = r.open('new'); // Actual use runs expiry; there is no timer or upkeep request.
  expect([...r.published().keys()].some((key) => key.endsWith('\u0000old'))).toBe(false);
  expect(owned.read().entries[0]?.name).toBe('kept');
  await fresh.release(); await owned.release();
});

it('reports expiry and a pressure-lost back trail as Restart without eager walking', async () => {
  const r = rig(), view = r.open();
  r.fetch.mockResolvedValueOnce(page(['one'], { next_cursor: 'two', truncated: true })); await view.first();
  r.fetch.mockResolvedValueOnce(page(['two'], { next_cursor: 'three', truncated: true })); await view.next();
  // Keep the exact current page owned; expire only the unowned first page/checkpoint.
  r.advance(DIRECTORY_IDLE_MS + 1); const other = r.open('other');
  expect(await view.previous()).toBe('restart');
  expect(view.read().entries[0]?.name).toBe('two'); expect(view.read().restartRequired).toBe(true);
  expect(r.fetch).toHaveBeenCalledTimes(2);
  await other.release(); await view.release();
  const expiring = r.open();
  r.fetch.mockResolvedValueOnce(page(['fresh'], { next_cursor: 'next', truncated: true,
    expires_at: new Date(Date.parse(STAMP) + DIRECTORY_IDLE_MS + 2).toISOString() })); await expiring.restart();
  r.advance(2);
  expect(expiring.read().canNext).toBe(false);
  expect(await expiring.next()).toBe('restart'); expect(r.fetch).toHaveBeenCalledTimes(3);
  await expiring.release();
});

it('joins final-owner cancellation while another pane retains its identical in-flight read', async () => {
  const r = rig(), left = r.open('', 'left'), right = r.open('', 'right');
  const gate = pending<DirListing>(); r.fetch.mockReturnValueOnce(gate.promise);
  const a = left.first(), b = right.first();
  expect(r.fetch).toHaveBeenCalledTimes(1);
  await left.release(); expect(r.fetch.mock.calls[0]?.[2].signal.aborted).toBe(false);
  let joined = false; const closing = right.release().then(() => { joined = true; });
  expect(r.fetch.mock.calls[0]?.[2].signal.aborted).toBe(true);
  await Promise.resolve(); expect(joined).toBe(false); expect(r.store.usage().active).toBe(1);
  gate.resolve(page(['retired'])); await Promise.all([a, b, closing]);
  expect(joined).toBe(true); expect(r.store.usage().active).toBe(0);
  expect(right.read().entries).toEqual([]);
});

it('does not credit an aborted fetch slot until true settlement, and cancels hidden queued work', async () => {
  const r = rig(), owners = Array.from({ length: 7 }, (_, index) => r.open(String(index)));
  const gates = Array.from({ length: 4 }, () => pending<DirListing>());
  for (const gate of gates) r.fetch.mockReturnValueOnce(gate.promise);
  const reads = owners.map((owner) => owner.first());
  expect(r.store.usage()).toMatchObject({ active: 4, queued: 3 });
  const closes = owners.map((owner) => owner.release());
  expect(r.store.usage()).toMatchObject({ active: 4, queued: 0 });
  const fresh = r.open('fresh'); r.fetch.mockResolvedValueOnce(page(['fresh'], { path: 'fresh' })); const reading = fresh.first();
  expect(r.fetch).toHaveBeenCalledTimes(4);
  for (const [index, gate] of gates.entries()) gate.resolve(page(['old'], { path: String(index) }));
  await Promise.all([...reads, ...closes, reading]);
  expect(r.fetch).toHaveBeenCalledTimes(5); expect(fresh.read().entries[0]?.name).toBe('fresh');
  await fresh.release();
});

it('allows only current resident parent demand, and leaving that page retires its child watch', async () => {
  const r = rig(), parent = r.open();
  r.event({ reset: true, changed: [entry('child', '', 'dir')], revision: 'revision', next_cursor: 'next', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  const child = r.open('child', 'one', 'binding', parent);
  expect(r.store.open(door, 'not-resident', { pane: 'one', binding: 'binding', parent })).toMatchObject({ ok: false, reason: 'parent' });
  expect(r.store.open(door, 'child', { pane: 'other', binding: 'binding', parent })).toMatchObject({ ok: false, reason: 'parent' });
  r.fetch.mockResolvedValueOnce(page(['next'])); await parent.next();
  expect(r.subscriptions[1]?.unsubscribe).toHaveBeenCalledTimes(1);
  expect(child.read().entries).toEqual([]); expect(r.store.usage().admitted).toBe(1);
  await parent.release();
});

it('enforces encoded bytes even with few rows and preserves the other panes on refusal', async () => {
  const r = rig(), parent = Array.from({ length: 14 }, () => 'p'.repeat(240)).join('/');
  const owners = Array.from({ length: 5 }, (_, index) => r.open(`${parent}/${index}`));
  // Each page is below the native serialized-page bound, but five exceed the shared payload budget.
  for (let index = 0; index < owners.length; index += 1) r.event({ reset: true,
    changed: Array.from({ length: 500 }, (_, row) => entry(String(row).padStart(4, '0'), `${parent}/${index}`)), revision: String(index) }, index);
  for (const owner of owners.slice(0, 4)) expect(owner.read().entries).toHaveLength(500);
  expect(owners[4]?.read().capacity?.reason).toBe('bytes');
  expect(r.store.usage().rows).toBe(2000); expect(r.store.usage().bytes).toBeLessThanOrEqual(8 * 1024 * 1024);
  await Promise.all(owners.map((owner) => owner.release()));
});

it('does not share an HTTP selector or its page through a different thread Door', async () => {
  const r = rig(), left = r.open('', 'left');
  const secondDoor = { ...door, threadId: 'other-thread' };
  const opened = r.store.open(secondDoor, '', { pane: 'right', binding: 'binding' });
  if (!opened.ok) throw new Error('Expected bounded second pane');
  const right = opened.lease, gate = pending<DirListing>(); r.fetch.mockReturnValueOnce(gate.promise);
  const reading = left.first(); expect(await right.first()).toBe('busy');
  gate.resolve(page(['left'])); await reading;
  expect(right.read().entries).toEqual([]); expect(r.subscriptions).toHaveLength(1);
  r.fetch.mockResolvedValueOnce(page(['right'], { revision: 'other' })); await right.first();
  expect(r.fetch.mock.calls[1]?.[0]).toEqual(secondDoor);
  expect(right.read().entries[0]?.name).toBe('right');
  expect(left.read().entries[0]?.name).toBe('left'); expect(left.read().restartRequired).toBe(true);
  await left.release(); await right.release();
});

it('joins an old owner after forget without targeting or cancelling its replacement', async () => {
  const r = rig(), old = r.open();
  const gate = pending<DirListing>(); r.fetch.mockReturnValueOnce(gate.promise); const reading = old.first();
  r.store.forget(door.environmentId);
  const replacement = r.open('', 'new', 'new-binding'); r.event({ reset: true, changed: [entry('new')] }, 1);
  let joined = false; const closing = old.release().then(() => { joined = true; });
  await Promise.resolve(); expect(joined).toBe(false);
  expect(r.subscriptions[1]?.unsubscribe).not.toHaveBeenCalled();
  gate.resolve(page(['old'])); await Promise.all([reading, closing]);
  expect(replacement.read().entries[0]?.name).toBe('new');
  await replacement.release();
});

it('joins a child read already retired by page departure when its parent finally closes', async () => {
  const r = rig(), parent = r.open();
  r.event({ reset: true, changed: [entry('child', '', 'dir')], revision: 'revision', next_cursor: 'next', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  const child = r.open('child', 'one', 'binding', parent), gate = pending<DirListing>();
  r.fetch.mockReturnValueOnce(gate.promise); const reading = child.first();
  r.fetch.mockResolvedValueOnce(page(['new-page'])); await parent.next();
  expect(child.read().entries).toEqual([]); expect(r.fetch.mock.calls[0]?.[2].signal.aborted).toBe(true);
  let joined = false; const closing = parent.release().then(() => { joined = true; });
  await Promise.resolve(); expect(joined).toBe(false);
  gate.resolve(page(['old'], { path: 'child' })); await Promise.all([reading, closing]);
  expect(joined).toBe(true); expect(r.store.usage().active).toBe(0);
});

it('transfers only the departing page pin under a full budget and truthfully loses page-one Previous', async () => {
  const r = rig(), owners = Array.from({ length: 4 }, (_, index) => r.open(String(index)));
  for (let index = 0; index < owners.length; index += 1) r.event({ reset: true,
    changed: Array.from({ length: 5000 }, (_, row) => entry(String(row), String(index))), revision: String(index),
    next_cursor: index === 0 ? 'next' : null, truncated: index === 0, as_of: STAMP, observed_from: STAMP }, index);
  const first = owners[0]; if (first === undefined) throw new Error('Expected owner');
  r.fetch.mockResolvedValueOnce(page(Array.from({ length: 5000 }, (_, row) => `next-${row}`), { path: '0', revision: '0' }));
  expect(await first.next()).toBe('done'); expect(r.store.usage().rows).toBe(20_000);
  expect(first.read().entries[0]?.name).toBe('next-0');
  expect(first.read()).toMatchObject({ canPrevious: false, restartRequired: true });
  expect(await first.previous()).toBe('restart'); expect(r.fetch).toHaveBeenCalledTimes(1);
  for (const owner of owners.slice(1)) expect(owner.read().entries).toHaveLength(5000);
  await Promise.all(owners.map((owner) => owner.release()));
});

it('page departure in one pane never retires the other pane\'s shared child demand', async () => {
  const r = rig(), left = r.open('', 'left'), right = r.open('', 'right');
  r.event({ reset: true, changed: [entry('child', '', 'dir')], revision: 'revision', next_cursor: 'next', truncated: true,
    as_of: STAMP, observed_from: STAMP });
  const leftChild = r.open('child', 'left', 'binding', left), rightChild = r.open('child', 'right', 'binding', right);
  r.event({ reset: true, changed: [entry('kept', 'child')] }, 1);
  r.fetch.mockResolvedValueOnce(page(['next-page'])); await left.next();
  expect(leftChild.read().entries).toEqual([]); expect(rightChild.read().entries[0]?.name).toBe('kept');
  expect(r.subscriptions).toHaveLength(2); expect(r.subscriptions[1]?.unsubscribe).not.toHaveBeenCalled();
  await left.release(); expect(r.subscriptions[1]?.unsubscribe).not.toHaveBeenCalled();
  await right.release(); expect(r.subscriptions[1]?.unsubscribe).toHaveBeenCalledTimes(1);
});

it('cannot renew or recreate an expired unowned directory with a cancelled late result', async () => {
  const r = rig(), old = r.open('old'), gate = pending<DirListing>();
  r.fetch.mockReturnValueOnce(gate.promise); const reading = old.first(), closing = old.release();
  r.advance(DIRECTORY_IDLE_MS + 1); const current = r.open('current');
  expect([...r.published().keys()].some((key) => key.endsWith('\u0000old'))).toBe(false);
  gate.resolve(page(['late'], { path: 'old' })); await Promise.all([reading, closing]);
  expect([...r.published().keys()].some((key) => key.endsWith('\u0000old'))).toBe(false);
  expect(r.store.usage().active).toBe(0); await current.release();
});

it('retires a finished ownerless legacy action observation before opening the actual pane binding', async () => {
  const r = rig(); r.fetch.mockResolvedValueOnce(page(['legacy-only']));
  await r.store.load(door, '', { source: 'live' });
  expect(r.store.usage().admitted).toBe(0);
  const pane = r.open('', 'one', 'current-source');
  expect(pane.read().entries).toEqual([]); expect(pane.read().watch).toBe('connecting');
  r.fetch.mockResolvedValueOnce(page(['current-source'])); await pane.first();
  expect(pane.read().entries[0]?.name).toBe('current-source');
  expect(r.fetch).toHaveBeenCalledTimes(2); expect(r.subscriptions).toHaveLength(1);
  await pane.release();
});

it('does not retire a legacy explicit action while its actual request is still owned', async () => {
  const r = rig(); r.fetch.mockResolvedValueOnce(page(['kept-legacy'])); await r.store.load(door, '');
  const gate = pending<DirListing>(); r.fetch.mockReturnValueOnce(gate.promise);
  const action = r.store.load(door, '', { source: 'live' });
  expect(r.store.open(door, '', { pane: 'one', binding: 'current-source' })).toMatchObject({ ok: false, reason: 'binding' });
  expect(r.fetch.mock.calls[1]?.[2].signal.aborted).toBe(false);
  expect([...r.published().values()][0]?.entries[0]?.name).toBe('kept-legacy');
  expect(r.subscriptions).toHaveLength(0);
  gate.resolve(page(['finished-legacy'])); await action;
  const pane = r.open('', 'one', 'current-source'); expect(pane.read().entries).toEqual([]);
  r.event({ reset: true, changed: [entry('fresh-current')] });
  expect(pane.read().entries[0]?.name).toBe('fresh-current'); await pane.release();
});

it('does not adopt an actively watched legacy observation and ignores its old frames after release', async () => {
  const r = rig(), releaseLegacy = r.store.watch(door, '');
  r.event({ reset: true, changed: [entry('legacy-owner')] });
  expect(r.store.open(door, '', { pane: 'one', binding: 'current-source' })).toMatchObject({ ok: false, reason: 'binding' });
  expect(r.subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  const old = r.subscriptions[0]; releaseLegacy();
  const pane = r.open('', 'one', 'current-source');
  r.event({ reset: true, changed: [entry('current-owner')] }, 1);
  old?.onFrame({ op: 'event', id: 1, feed: 'old', data: { reset: true, changed: [entry('late-legacy')] } });
  expect(pane.read().entries[0]?.name).toBe('current-owner');
  expect(old?.unsubscribe).toHaveBeenCalledTimes(1); await pane.release();
});

it('never steals an unowned real binding and a stale consumer cannot undo the common rebase', async () => {
  const r = rig(), old = r.open('', 'one', 'source-a');
  r.event({ reset: true, changed: [entry('old-source')] }); await old.release();
  expect(r.store.open(door, '', { pane: 'two', binding: 'source-b' })).toMatchObject({ ok: false, reason: 'binding' });
  r.store.forget(door.environmentId);
  const current = r.open('', 'two', 'source-b'); r.event({ reset: true, changed: [entry('current-source')] }, 1);
  expect(r.store.open(door, '', { pane: 'one', binding: 'source-a' })).toMatchObject({ ok: false, reason: 'binding' });
  await old.release(); expect(current.read().entries[0]?.name).toBe('current-source');
  expect(r.subscriptions[1]?.unsubscribe).not.toHaveBeenCalled(); await current.release();
});

it('keeps FileMenu explicit refreshes inside an already established common pane binding', async () => {
  const r = rig(), pane = r.open('', 'one', 'current-source');
  r.event({ reset: true, changed: [entry('before-action')] });
  r.fetch.mockResolvedValueOnce(page(['after-action'])); await r.store.load(door, '', { source: 'live' });
  expect(pane.read().entries[0]?.name).toBe('after-action');
  const other = r.open('', 'two', 'current-source');
  expect(other.read().entries[0]?.name).toBe('after-action');
  expect(r.subscriptions).toHaveLength(1); expect(r.subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  await other.release(); await pane.release();
});
