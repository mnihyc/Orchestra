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

it('refreshes a shown place only when the capacity of a machine it may read changes', async () => {
  const cache = new QueryClient();
  const key = ['start-options', 'topic', ''];
  const capacity = { cpus: 4, memory_bytes: 4000, data_volume_bytes: 8000, capacity_refresh_secs: 600 };
  const restricted = { machine: { id: 'quiet' } };
  const choices = (memory_bytes: number) => ({
    environments: [{ machine: { id: 'visible' }, capacity: { ...capacity, memory_bytes } }, restricted],
  });
  const read = vi.fn(() => Promise.resolve(choices(3600)));
  const stop = new QueryObserver(cache, { queryKey: key, initialData: choices(4000), staleTime: Infinity, queryFn: read }).subscribe(() => undefined);
  try {
    expect(wantedResourceFeeds(cache)).toEqual(new Set(['machine:visible']));
    for (const id of ['visible', 'quiet']) applyRow(cache, { kind: 'machine', id, value: { id, facts: { ...capacity, rates: { up_bps: 10 } } } });
    expect(read).not.toHaveBeenCalled();
    applyRow(cache, { kind: 'machine', id: 'visible', value: { id: 'visible', facts: { ...capacity, memory_bytes: 3600 } } });
    await vi.waitFor(() => expect(cache.getQueryData(key)).toEqual(choices(3600)));
    expect(read).toHaveBeenCalledOnce();
  } finally {
    stop();
    cache.clear();
  }
});

it('updates an open native system review when the connector reports a new executable', async () => {
  const cache = new QueryClient();
  const key = ['machine-policy', 'laboratory'];
  const build = (executable: string) => ({ os: 'windows', arch: 'x86_64', executable });
  const before = build('a'.repeat(64)), after = build('b'.repeat(64));
  const first = { platform_review: { build: before, approved: true, approval: null } };
  const next = { platform_review: { build: after, approved: false, approval: null } };
  const read = vi.fn(() => Promise.resolve(next));
  const stop = new QueryObserver(cache, { queryKey: key, initialData: first, staleTime: Infinity, queryFn: read }).subscribe(() => undefined);
  try {
    expect(wantedResourceFeeds(cache)).toContain('machine:laboratory');
    const report = (running: typeof before) => ({ id: 'laboratory', facts: { runtime_platform: running, policy: { 'exec.platform': { state: 'approved', build: before } } } });
    applyRow(cache, { kind: 'machine', id: 'laboratory', value: report(before) });
    expect(read).not.toHaveBeenCalled();
    applyRow(cache, { kind: 'machine', id: 'laboratory', value: report(after) });
    await vi.waitFor(() => expect(cache.getQueryData(key)).toEqual(next));
    expect(read).toHaveBeenCalledOnce();
  } finally { stop(); cache.clear(); }
});
