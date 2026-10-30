/** Import requests keep source paths and use the same act identity on a retry. */
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { downloadImportReport, importForJob, importKeys, planImport, startImport, startImportUndo } from '@/api/imports';
import type { ImportRequest } from '@/generated/api-types';

const calls = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }));
vi.mock('@/api/client', async (original) => ({ ...await original<object>(), api: { GET: calls.get, POST: calls.post } }));
afterEach(() => { calls.get.mockReset(); calls.post.mockReset(); });

const request: ImportRequest = {
  client_id: 'import-act', source: { files: [{ name: 'Notes/images/map.png', blob: 'a'.repeat(64) }] },
  destination: {}, options: {}, choices: {},
};

it('asks for the requested page of a dry run with the full source paths', async () => {
  calls.post.mockResolvedValue({ data: { counts: {} }, response: new Response(null, { status: 200 }) });
  await planImport('place', request, { cursor: 'next', conflicts_only: true });
  expect(calls.post).toHaveBeenCalledExactlyOnceWith('/nodes/{id}/imports/preflight', {
    params: { path: { id: 'place' } }, body: { request, view: { limit: 50, cursor: 'next', conflicts_only: true } },
  });
});

it('reuses the act identity for an import and its undo', async () => {
  calls.post.mockResolvedValue({ data: { id: 'job' }, response: new Response(null, { status: 202 }) });
  await startImport('place', request);
  await startImport('place', request);
  await startImportUndo('import', 'undo-act');
  expect(calls.post.mock.calls[0]).toEqual(calls.post.mock.calls[1]);
  expect(calls.post.mock.calls[0]?.[1]).toMatchObject({ body: request, headers: { 'Idempotency-Key': 'import-act' } });
  expect(calls.post.mock.calls[2]?.[1]).toMatchObject({ body: { client_id: 'undo-act' }, headers: { 'Idempotency-Key': 'undo-act' } });
});

it('downloads a report with its server-supplied name and measured progress', async () => {
  const bytes = new TextEncoder().encode('name,outcome\nPlan,created\n');
  const stream = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(bytes); controller.close(); } });
  calls.get.mockResolvedValue({ data: stream, response: new Response(null, { status: 200, headers: { 'Content-Length': String(bytes.length), 'Content-Disposition': 'attachment; filename="report.csv"' } }) });
  const progress: { received: number; total: number | null }[] = [];
  const chunks: Uint8Array[] = [];
  const result = await downloadImportReport('import', 'csv', (value) => progress.push(value), (chunk) => { chunks.push(chunk); return Promise.resolve(); });
  expect(result.name).toBe('report.csv');
  expect(result.bytes).toBe(bytes.length);
  expect(chunks).toEqual([bytes]);
  expect(progress).toEqual([{ received: 0, total: bytes.length }, { received: bytes.length, total: bytes.length }]);
  expect(calls.get).toHaveBeenCalledExactlyOnceWith('/imports/{id}/report/download', { params: { path: { id: 'import' }, query: { format: 'csv' } }, parseAs: 'stream' });
});

it('does not scan an old job through an unbounded history', async () => {
  calls.get.mockResolvedValueOnce({ data: { items: [], next_cursor: 'next' }, response: new Response(null, { status: 200 }) });
  expect(await importForJob('job')).toBeNull();
  expect(calls.get).toHaveBeenCalledExactlyOnceWith('/me/imports', { params: { query: { limit: 50 } } });
});

it('does not reuse import cache entries for a different reader', () => {
  const before = client.session.getState();
  const keys = () => [importKeys.mine(), importKeys.one('import'), importKeys.report('import', {}), importKeys.inspection('place', request),
    importKeys.plan('place', request, {}), importKeys.preview('place', { source: request.source, object: 'source' }), importKeys.undo('import'), importKeys.forJob('job')];
  try {
    client.session.setState({ ...before, userId: 'first', readerVersion: 1, status: 'signed_in' });
    const first = keys();
    client.session.setState({ userId: 'second', readerVersion: 2, status: 'signed_in' });
    keys().forEach((key, index) => expect(key).not.toEqual(first[index]));
  } finally { client.session.setState(before); }
});
