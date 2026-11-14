/** Embedded provider facts follow their existing feed without HTTP reads and withdraw with their own access. */
import { QueryClient, QueryClientProvider, QueryObserver } from '@tanstack/react-query';
import { render, renderHook, waitFor } from '@testing-library/react';
import { createElement, type ReactNode } from 'react';
import { expect, it, vi } from 'vitest';
import { applyRow } from '@/api/cache';
import { wantedResourceFeeds } from '@/realtime/resourceWants';
import { planProvider } from './provider-plan-fixtures';
import { socket } from '@/realtime/live';
import { useResourceSample } from '@/features/resource-facts/live';
import { KeptQueryScope } from '@/features/workbench/KeptQueries';

it('shares one current resource subscription across retained views of the same cache', () => {
  const root = new QueryClient(), scoped = new QueryClient({ queryCache: root.getQueryCache() }), stopped = vi.fn();
  const original = socket.subscribe.bind(socket);
  const subscribe = vi.spyOn(socket, 'subscribe').mockImplementation((...args) => {
    const native = original(...args);
    return { ...native, unsubscribe: () => { stopped(); native.unsubscribe(); } };
  });
  const wrapper = (cache: QueryClient) => ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: cache, children });
  const one = renderHook(() => useResourceSample('shared-sample'), { wrapper: wrapper(root) });
  const two = renderHook(() => useResourceSample('shared-sample'), { wrapper: wrapper(scoped) });
  try {
    expect(subscribe).toHaveBeenCalledOnce();
    one.unmount(); expect(stopped).not.toHaveBeenCalled();
    two.unmount(); expect(stopped).toHaveBeenCalledOnce();
  } finally { one.unmount(); two.unmount(); root.clear(); subscribe.mockRestore(); }
});

it('stops current resource subscriptions while a retained local act keeps hidden effects mounted', async () => {
  const root = new QueryClient(), stopped = vi.fn(), original = socket.subscribe.bind(socket);
  const subscribe = vi.spyOn(socket, 'subscribe').mockImplementation((...args) => {
    const native = original(...args);
    return { ...native, unsubscribe: () => { stopped(); native.unsubscribe(); } };
  });
  function Sample() { useResourceSample('held-sample'); return null; }
  const tree = (shown: boolean) => createElement(QueryClientProvider, { client: root,
    children: createElement(KeptQueryScope, { shown, children: createElement(Sample) }) });
  const view = render(tree(true));
  try {
    expect(subscribe).toHaveBeenCalledOnce();
    view.rerender(tree(false)); expect(stopped).toHaveBeenCalledOnce();
    view.rerender(tree(true)); expect(subscribe).toHaveBeenCalledOnce();
    await waitFor(() => expect(subscribe).toHaveBeenCalledTimes(2));
  } finally { view.unmount(); root.clear(); subscribe.mockRestore(); }
});

it('follows one provider for multiple shown thread parameter projections and applies live values', () => {
  const cache = new QueryClient();
  const read = vi.fn();
  const provider = planProvider();
  const stops = ['one', 'two'].map((id) => new QueryObserver(cache, {
    queryKey: ['thread-parameters', id], initialData: { provider }, staleTime: 30_000,
    queryFn: () => { read(); return Promise.resolve({ provider }); },
  }).subscribe(() => undefined));
  expect(wantedResourceFeeds(cache)).toEqual(new Set([`provider:${provider.id}`]));
  const updated = structuredClone(provider);
  updated.facts.windows[1]!.used_percent = 87;
  applyRow(cache, { kind: 'provider', id: provider.id, value: { ...updated } });
  for (const id of ['one', 'two']) expect(cache.getQueryData<{ provider: typeof provider }>(['thread-parameters', id])?.provider.facts.windows[1]?.used_percent).toBe(87);
  applyRow(cache, { kind: 'provider', id: provider.id, value: null });
  expect(wantedResourceFeeds(cache)).toEqual(new Set());
  for (const id of ['one', 'two']) expect(cache.getQueryData<{ provider: typeof provider | null }>(['thread-parameters', id])?.provider).toBeNull();
  expect(read).not.toHaveBeenCalled();
  for (const stop of stops) stop();
  cache.clear();
});

it('never follows provider facts from an administrative-only or absent projection', () => {
  const cache = new QueryClient();
  const provider = planProvider(); provider.access.permissions = [];
  const stop = new QueryObserver(cache, { queryKey: ['thread-parameters', 'reader'], initialData: { provider }, staleTime: 30_000, queryFn: () => Promise.resolve({ provider }) }).subscribe(() => undefined);
  expect(wantedResourceFeeds(cache)).toEqual(new Set());
  stop(); cache.clear();
});
