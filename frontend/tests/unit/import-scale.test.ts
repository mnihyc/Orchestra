/** Large sources, histories and reports keep work, requests and streamed bytes bounded. */
import { InfiniteQueryObserver, QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';

import { downloadImportReport, importKeys, importReportQuery } from '@/api/imports';
import { readImportSource, SOURCE_BATCH, type ImportSource } from '@/features/people/importSource';
import { ImportUploads } from '@/features/people/importUploads';
import type { ImportRequest } from '@/generated/api-types';

const calls = vi.hoisted(() => ({ get: vi.fn() }));
vi.mock('@/api/client', async (original) => ({ ...await original<object>(), api: { GET: calls.get } }));
afterEach(() => { calls.get.mockReset(); });

it('checks 100,000 paths in bounded turns and can cancel between turns', async () => {
  let checked = 0, last = 0, turns = 0;
  const files = Array.from({ length: 100_000 }, (_, index) => ({
    get webkitRelativePath() { checked += 1; return `Notes/${index}.md`; }, size: 1,
  } as File));
  const source = await readImportSource(files, false, new AbortController().signal, () => {
    expect(checked - last).toBeLessThanOrEqual(SOURCE_BATCH);
    last = checked; turns += 1;
    return Promise.resolve();
  });
  expect(source.fault).toBeNull();
  expect(source.files).toHaveLength(100_000);
  expect(source.bytes).toBe(100_000);
  expect(turns).toBe(Math.floor(100_000 / SOURCE_BATCH));
  const stop = new AbortController();
  checked = 0;
  await expect(readImportSource(files, false, stop.signal, () => { stop.abort(); return Promise.resolve(); })).rejects.toMatchObject({ name: 'AbortError' });
  expect(checked).toBe(SOURCE_BATCH);
});

it('allocates one task and reads one entry when the first upload fails in a million-file source', async () => {
  let entries = 0;
  const entry = { file: new File(['x'], 'one.md'), path: 'one.md' };
  const files = new Proxy(new Array< typeof entry >(1_000_000), { get(target, key, receiver) {
    if (typeof key === 'string' && /^\d+$/.test(key)) { entries += 1; return entry; }
    const value: unknown = Reflect.get(target, key, receiver);
    return value;
  } });
  const source: Extract<ImportSource, { fault: null }> = { files, bytes: 1_000_000, fault: null };
  const cancel = vi.fn(() => Promise.resolve());
  const make = vi.fn(() => ({ run: () => Promise.reject(new Error('Connection lost')), cancel }));
  const batch = new ImportUploads(source, make);
  expect(entries).toBe(0);
  expect(make).not.toHaveBeenCalled();
  await expect(batch.run(new AbortController().signal, () => undefined)).rejects.toThrow('Connection lost');
  expect(entries).toBe(1);
  expect(make).toHaveBeenCalledOnce();
  await batch.cancel();
  expect(cancel).toHaveBeenCalledExactlyOnceWith({ keepCompleted: true });
  expect(entries).toBe(1);
});

it('retains one 50-row page of a million-row report and makes no speculative requests', async () => {
  const total = 1_000_000;
  calls.get.mockImplementation((_path: string, options: { params: { query: { limit: number; cursor?: string } } }) => {
    const { limit, cursor } = options.params.query;
    const offset = Number(cursor ?? 0);
    expect(limit).toBe(50);
    return Promise.resolve({ response: new Response(null, { status: 200 }), data: {
      items: Array.from({ length: limit }, (_, index) => ({ position: offset + index, name: `Row ${offset + index}` })),
      next_cursor: offset + limit < total ? String(offset + limit) : null,
    } });
  });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const options = importReportQuery('large-report', {});
  const observer = new InfiniteQueryObserver(cache, options);
  try {
    await cache.infiniteQuery(options);
    expect(calls.get).toHaveBeenCalledTimes(1);
    const next = await observer.fetchNextPage();
    expect(calls.get).toHaveBeenCalledTimes(2);
    expect(next.data?.pages).toHaveLength(1);
    expect(next.data?.pages[0]?.items).toHaveLength(50);
    expect(next.data?.pages[0]?.items[0]?.position).toBe(50);
    expect(next.data?.pageParams).toEqual(['50']);
  } finally { observer.destroy(); cache.clear(); }
});

it('writes a report larger than 6 GiB with one awaited chunk and no byte accumulation', async () => {
  const chunks = 100_000, piece = new Uint8Array(65_536);
  let produced = 0, consumed = 0, outstanding = 0, written = 0, active = 0, peak = 0, measured = 0, progress = 0, wrong = 0;
  vi.spyOn(performance, 'now').mockReturnValue(0);
  const stream = new ReadableStream<Uint8Array>({ pull(controller) {
    if (produced === chunks) { controller.close(); return; }
    produced += 1; outstanding = Math.max(outstanding, produced - consumed); controller.enqueue(piece);
  } });
  calls.get.mockResolvedValue({ data: stream, response: new Response(null, { status: 200 }) });
  const file = await downloadImportReport('large-report', 'json', (value) => { measured = value.received; progress += 1; }, async (bytes) => {
    active += 1; peak = Math.max(peak, active);
    if (bytes !== piece) wrong += 1;
    await Promise.resolve();
    written += bytes.byteLength; consumed += 1; active -= 1;
  });
  expect(peak).toBe(1);
  // The response may prefetch one chunk while the chosen file writes the current chunk.
  expect(outstanding).toBeLessThanOrEqual(2);
  expect(wrong).toBe(0);
  expect(progress).toBe(2);
  expect(produced).toBe(chunks);
  expect(written).toBe(chunks * piece.byteLength);
  expect(measured).toBe(written);
  expect(file.bytes).toBe(written);
});

it('never walks source files while hashing plan and preview query keys', () => {
  let reads = 0;
  const source: ImportRequest['source'] = {
    get files(): ImportRequest['source']['files'] { reads += 1; throw new Error('The query key must not read a million files'); },
  };
  const request = { client_id: 'act', source, destination: {}, choices: {}, options: {} } as ImportRequest;
  JSON.stringify(importKeys.inspection('place', request));
  JSON.stringify(importKeys.plan('place', request, {}));
  JSON.stringify(importKeys.preview('place', { source, object: 'one' }));
  expect(reads).toBe(0);
  expect(importKeys.plan('place', { ...request }, {})).not.toEqual(importKeys.plan('place', request, {}));
});
