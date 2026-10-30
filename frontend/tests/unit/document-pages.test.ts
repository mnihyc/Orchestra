// @vitest-environment node
/** Visible first pages share transport without sharing cancellation, permission or reader authority. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as ClientModule from '@/api/client';
import { firstDocumentPage } from '@/api/documentPages';
import type { NodeDocumentPage, Problem } from '@/generated/api-types';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options: { signal: AbortSignal; params: { path: { id: string }; query: { node_ids: string; limit: number } } }) => Promise<NodeDocumentPage[]>>() }));
vi.mock('@/api/client', async (original) => ({ ...await original<typeof ClientModule>(),
  api: { GET: calls.get }, ok: (reply: Promise<unknown>) => reply,
}));
const before = client.session.getState();
const page = { items: [], next_cursor: null };
const reply = (id: string): NodeDocumentPage => ({ node_id: id, page, error: null });
beforeEach(() => { calls.get.mockReset(); client.session.setState({ readerVersion: 7 }); });
afterEach(() => { client.session.setState(before); });

it('reads simultaneous visible places once while keeping their native result identities', async () => {
  calls.get.mockResolvedValue([reply('one'), reply('two')]);
  const signal = new AbortController().signal;
  expect(await Promise.all([firstDocumentPage('one', 30, signal), firstDocumentPage('two', 30, signal)])).toEqual([page, page]);
  expect(calls.get).toHaveBeenCalledOnce();
  expect(calls.get.mock.calls[0]).toMatchObject(['/nodes/{id}/document-pages', { params: { path: { id: 'one' }, query: { node_ids: 'one,two', limit: 30 } } }]);
});

it('keeps one place’s native refusal apart from another readable page', async () => {
  const error: Problem = { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Not allowed', status: 403, detail: 'No access', request_id: 'denied' };
  calls.get.mockResolvedValue([reply('one'), { node_id: 'two', page: null, error }]);
  const signal = new AbortController().signal;
  const results = await Promise.allSettled([firstDocumentPage('one', 30, signal), firstDocumentPage('two', 30, signal)]);
  expect(results[0]).toEqual({ status: 'fulfilled', value: page });
  expect(results[1]).toMatchObject({ status: 'rejected', reason: { problem: error } });
});

it('does not cancel another visible place when one observer closes', async () => {
  let finish!: (pages: NodeDocumentPage[]) => void;
  calls.get.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const one = new AbortController(), two = new AbortController();
  const results = Promise.allSettled([firstDocumentPage('one', 30, one.signal), firstDocumentPage('two', 30, two.signal)]);
  await Promise.resolve(); one.abort();
  expect(calls.get.mock.calls[0]?.[1].signal.aborted).toBe(false);
  finish([reply('one'), reply('two')]);
  expect(await results).toMatchObject([{ status: 'rejected', reason: { name: 'AbortError' } }, { status: 'fulfilled', value: page }]);
});

it('fences a late batch response after the session reader is replaced', async () => {
  let finish!: (pages: NodeDocumentPage[]) => void;
  calls.get.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const signal = new AbortController().signal;
  const results = Promise.allSettled([firstDocumentPage('one', 30, signal), firstDocumentPage('two', 30, signal)]);
  await Promise.resolve(); client.session.setState({ readerVersion: 8 }); finish([reply('one'), reply('two')]);
  expect(await results).toMatchObject([{ status: 'rejected', reason: { name: 'AbortError' } }, { status: 'rejected', reason: { name: 'AbortError' } }]);
});

it('drains a hundred thousand requested places in bounded native batches without losing their cursors', async () => {
  // This is the lower synthetic-world size; 200 is the endpoint's existing place bound.
  const places = 100_000, nativeBatch = 200;
  let active = 0, peak = 0, largest = 0, delivered = 0;
  calls.get.mockImplementation(async (_path, options) => {
    active += 1; peak = Math.max(peak, active);
    const ids = options.params.query.node_ids.split(',');
    largest = Math.max(largest, ids.length);
    await Promise.resolve();
    active -= 1; delivered += ids.length;
    return ids.map((id) => ({ node_id: id, page: { ...page, next_cursor: `next:${id}` }, error: null }));
  });
  const signal = new AbortController().signal;
  const results = await Promise.all(Array.from({ length: places }, (_, index) => firstDocumentPage(String(index), 30, signal)));
  expect(delivered).toBe(places);
  expect(largest).toBeLessThanOrEqual(nativeBatch);
  expect(peak).toBe(1);
  expect(calls.get).toHaveBeenCalledTimes(places / nativeBatch);
  expect(results.every((result, index) => result.next_cursor === `next:${index}`)).toBe(true);
}, 30_000);
