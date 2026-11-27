/** The left bar asks for its tree again only when the order the server keeps for the reader changes, not when it arrives. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { expect, it } from 'vitest';

import { meStateKey } from '@/api/personal';
import { settingsKey } from '@/api/settings';
import { useTreeOrder } from '@/features/navigation/treeOrder';

it('names no order of the tree until both its parts have arrived, then a new one only when the reader changes it', async () => {
  // Nothing here is read from a server: each part arrives when the case says.
  const cache = new QueryClient({ defaultOptions: { queries: { enabled: false, retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={cache}>{children}</QueryClientProvider>;
  const hook = renderHook(() => useTreeOrder('org-1').revision, { wrapper });
  expect(hook.result.current).toBeUndefined();
  await act(async () => { cache.setQueryData(meStateKey, { sidebar: {} }); await new Promise((resolve) => setTimeout(resolve, 20)); });
  expect(hook.result.current).toBeUndefined();
  act(() => { cache.setQueryData(settingsKey('org-1'), [{ key: 'navigation.order', value: { by: 'changed' } }]); });
  await waitFor(() => { expect(hook.result.current).toBeDefined(); });
  const first = hook.result.current;
  hook.rerender();
  expect(hook.result.current).toBe(first);
  act(() => { cache.setQueryData(meStateKey, { sidebar: { order: { 'org-1': { by: 'name' } } } }); });
  await waitFor(() => { expect(hook.result.current).not.toBe(first); });
  expect(hook.result.current).toBeDefined();
  hook.unmount();
  cache.clear();
});
