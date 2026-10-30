/** A machine report repairs only the binding choices that were waiting for that agent. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, resourceChanges } from '@/api/acts';
import type { BindingOptions } from '@/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { World } from '../doubles/world';

for (const reported of [true, false]) it(`updates visible choices when the agent becomes ${reported ? 'reported' : 'unreported'}, leaving hidden and unrelated choices unread`, async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const id = RUNS_ON.codex;
  world.resources.report(id, !reported);
  const choices = () => world.starts.handle(person, 'GET', `/nodes/${ACME.fixLogin}/binding/options`, new URLSearchParams(), {})!.body as BindingOptions;
  const before = choices();
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const visible = ['start-options', ACME.fixLogin, ''];
  const hidden = ['start-options', ACME.web, ''];
  const unrelated = ['start-options', 'another-place', ''];
  const other = { ...before, providers: before.providers.map((provider) => ({ ...provider, agents: provider.agents.filter((agent) => agent.agent.id !== id) })) };
  cache.setQueryData(visible, before);
  cache.setQueryData(hidden, before);
  cache.setQueryData(unrelated, other);
  const read = vi.fn(() => Promise.resolve(choices()));
  const readOther = vi.fn(() => Promise.resolve(other));
  const stop = new QueryObserver(cache, { queryKey: visible, queryFn: read }).subscribe(() => undefined);
  const stopOther = new QueryObserver(cache, { queryKey: unrelated, queryFn: readOther }).subscribe(() => undefined);
  try {
    world.resources.report(id, reported);
    const row = world.resources.inventoryView(person, 'agent', id);
    if (row === null) throw new Error('Missing agent fixture');
    const change = resourceChanges('agent', id, { row }, 'news');
    await Promise.all([applyChanges(change, cache), applyChanges(change, cache)]);
    expect(cache.getQueryData(visible)).toEqual(choices());
    expect(cache.getQueryData(hidden)).toBeUndefined();
    expect(cache.getQueryData(unrelated)).toEqual(other);
    expect(read).toHaveBeenCalledTimes(1);
    expect(readOther).not.toHaveBeenCalled();
    await applyChanges(change, cache);
    expect(read).toHaveBeenCalledTimes(1);
  } finally { stop(); stopOther(); cache.clear(); }
});
