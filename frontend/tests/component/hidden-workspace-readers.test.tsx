/** Kept hidden views must not stat, fetch or read a file, including after an invalidation. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Workspace from '@/api/workspace';
import { useFileBytes, useFileHead, useFileText } from '@/features/files/content';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { show } from './support';

const calls = vi.hoisted(() => ({ stat: vi.fn(), fetch: vi.fn(), bytes: vi.fn() }));
vi.mock('@/api/workspace', async (importOriginal) => ({
  ...(await importOriginal<typeof Workspace>()),
  statFile:calls.stat,fetchFile:calls.fetch,readFileBytes:calls.bytes,
}));
afterEach(() => { cleanup();vi.clearAllMocks(); });
const door = { environmentId: 'environment',threadId: 'thread' };
const ready = { state:'ready' as const,hash:'kept-content',size:4 };
function Reader({ path }: { path:string }) {
  const head = useFileHead(door,path);
  const text = useFileText(door,path,ready);
  useFileBytes(door,path,ready);
  return <output>{head.head === undefined ? 'Waiting' : text.text?.text ?? 'Reading'}</output>;
}

it('starts machine-backed reads only while shown and keeps a hidden new path quiet', async () => {
  calls.stat.mockResolvedValue({ name:'code.txt' });
  calls.fetch.mockResolvedValue({ state:'ready',hash:ready.hash,size:4 });
  calls.bytes.mockResolvedValue({ bytes:new TextEncoder().encode('code'),total:4 });
  const cache = new QueryClient({defaultOptions:{queries:{retry:false}}});
  const view = (shown:boolean,path='code.txt') => <QueryClientProvider client={cache}>
    <ViewOpeningContext.Provider value={shown?'opening':null}><Reader path={path} /></ViewOpeningContext.Provider>
  </QueryClientProvider>;
  const rendered = show(view(false));
  await act(async () => { await Promise.resolve(); });
  expect(calls.stat).not.toHaveBeenCalled();expect(calls.fetch).not.toHaveBeenCalled();expect(calls.bytes).not.toHaveBeenCalled();
  rendered.rerender(view(true));
  expect(await screen.findByText('code',{exact:true})).toBeVisible();
  expect(calls.stat).toHaveBeenCalledTimes(1);expect(calls.fetch).toHaveBeenCalledTimes(1);
  rendered.rerender(view(false,'other.txt'));
  vi.clearAllMocks();
  await act(async () => { await cache.invalidateQueries({queryKey:['workspace-head',door.environmentId],exact:false}); });
  expect(calls.stat).not.toHaveBeenCalled();expect(calls.fetch).not.toHaveBeenCalled();expect(calls.bytes).not.toHaveBeenCalled();
  rendered.unmount();cache.clear();
});
