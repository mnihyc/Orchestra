/** The bar's choices are read from the server only once the shell's first read, which carries them, has landed. */
import { QueryClient } from '@tanstack/react-query';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import { useSidebarPreferences } from '@/features/navigation/preferences';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { show } from './support';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: { signal?: AbortSignal }) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: (request: Promise<unknown>) => request };
});
beforeEach(() => { calls.get.mockReset().mockResolvedValue({ sidebar: { peek: true } }); });

function Probe() {
  const { peek } = useSidebarPreferences();
  return <p>{`peek ${String(peek)}`}</p>;
}
const around = (ready: boolean) => <FrameReadiness.Provider value={{ ready, pending: null }}><Probe /></FrameReadiness.Provider>;

it('asks for the saved state when the shell is ready, and not before', async () => {
  const view = show(around(false), new QueryClient({ defaultOptions: { queries: { retry: false } } }));
  // Give anything that would ask a turn of the event loop to do so.
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(calls.get).not.toHaveBeenCalled();
  view.rerender(around(true));
  await vi.waitFor(() => { expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me/state']); });
});
