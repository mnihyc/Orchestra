/** Embedded provider facts follow their existing feed without HTTP reads and withdraw with their own access. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyRow } from '@/api/cache';
import { wantedResourceFeeds } from '@/realtime/resourceWants';
import { planProvider } from './provider-plan-fixtures';

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
