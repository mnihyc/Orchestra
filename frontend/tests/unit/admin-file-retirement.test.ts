/** Administrative previews and derived bytes leave no surviving ordinary cache when reach retires. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Trace from '@/api/trace';
import { client } from '@/api/client';
import { applyChanges } from '@/api/acts';
import { queryClient } from '@/api/query';
import { bytesQuery } from '@/features/files/content';
import { previewQuery, sourceArchive, sourceArchiveEntry, sourceBytes, sourceDownload, sourceHeadQuery } from '@/features/files/source';
import { inventoryFile } from '@/features/uploads/Inventory';
import { reachUpload } from '../doubles/admin-reach';

const request = vi.hoisted(() => vi.fn<typeof Trace.traceRequest>());
vi.mock('@/api/trace', async (load) => ({ ...await load<typeof Trace>(), traceRequest: request }));
const before = client.session.getState();
const source = inventoryFile(reachUpload({ can_open: true, admin_access: { place: 'system', private: true } }), 'archive.zip', true);
if (source === null) throw new Error('Completed fixture must have a custody source');
beforeEach(() => {
  request.mockReset(); queryClient.setQueryData(['admin-mode'], { as_admin: true, available: true });
  queryClient.setQueryData(['me'], { is_system_admin: true });
});
afterEach(() => { queryClient.clear(); client.session.setState(before); });

it('retires both the native preview/head and the derived byte supplier on admin-mode loss', async () => {
  const c = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const options = [previewQuery(source), sourceHeadQuery(source), bytesQuery({ environmentId: '', threadId: null }, source.url, 'hash', undefined, source)];
  for (const row of options) {
    c.setQueryData(['admin-mode'], { available: true, as_admin: true });
    const read = vi.fn(() => Promise.resolve('Forbidden reread'));
    // A marker tests retirement of this exact supplier, not its native data projection.
    const key = [...row.queryKey];
    c.setQueryData(key, 'Old private bytes');
    const observer = new QueryObserver(c, { queryKey: key, meta: row.meta, queryFn: read, staleTime: Infinity });
    const stop = observer.subscribe(() => undefined);
    try {
      await applyChanges([{ session: true, mode: { available: true, as_admin: false } }], c);
      expect(c.getQueryData(key)).toBeUndefined(); expect(read).not.toHaveBeenCalled();
    } finally { stop(); observer.destroy(); }
  }
  c.clear();
});

it('uses the declared administrative archive entry query rather than an invented /entry path', async () => {
  request.mockResolvedValue(new Response('entry bytes'));
  const bytes = await sourceArchiveEntry(source, 'notes.txt');
  expect(new TextDecoder().decode(bytes)).toBe('entry bytes');
  expect(request.mock.calls[0]?.[0]).toBe(`${source.archiveUrl}&entry=notes.txt`);
});

it('rejects late bytes, archive metadata and downloads after mode is switched off', async () => {
  for (const read of [() => sourceBytes(source, 'hash'), () => sourceArchive(source, ''), () => sourceArchiveEntry(source, 'entry'), () => sourceDownload(source)]) {
    queryClient.setQueryData(['admin-mode'], { as_admin: true });
    request.mockImplementation(() => {
      queryClient.setQueryData(['admin-mode'], { as_admin: false });
      return Promise.resolve(new Response(JSON.stringify({ entries: [] })));
    });
    await expect(read()).rejects.toMatchObject({ name: 'AbortError' });
  }
});

it('does not send another administrative byte read after standing ends', async () => {
  queryClient.setQueryData(['me'], { is_system_admin: false });
  await expect(sourceArchiveEntry(source, 'entry')).rejects.toMatchObject({ name: 'AbortError' });
  expect(request).not.toHaveBeenCalled();
});
