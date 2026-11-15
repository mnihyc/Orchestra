/** A machine report repairs only the binding choices that were waiting for that agent. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { client } from '@/api/client';
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

it('repairs displayed choices when a newly reported agent was absent from every cached candidate list', async () => {
  const world = new World(), person = world.people.get('ada')!;
  const id = RUNS_ON.codex;
  const answer = () => world.starts.handle(person, 'GET', `/nodes/${ACME.fixLogin}/binding/options`, new URLSearchParams(), {})!.body as BindingOptions;
  const full = answer();
  const before = { ...full, providers: full.providers.map((provider) => ({ ...provider, agents: provider.agents.filter((candidate) => candidate.agent.id !== id) })) };
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['start-options', ACME.fixLogin, ''];
  cache.setQueryData(key, before);
  const read = vi.fn(() => Promise.resolve(answer()));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    world.resources.report(id, true);
    const row = world.resources.inventoryView(person, 'agent', id);
    if (row === null) throw new Error('The native agent report exists');
    await applyChanges(resourceChanges('agent', id, { row }, 'news'), cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toEqual(answer());
  } finally { stop(); cache.clear(); }
});

it('keeps a new reader’s agent repair when a former reader’s request settles afterward', async () => {
  const previous = client.session.getState().readerVersion;
  const world = new World(), person = world.people.get('ada')!, id = RUNS_ON.codex;
  const answer = () => world.starts.handle(person, 'GET', `/nodes/${ACME.fixLogin}/binding/options`, new URLSearchParams(), {})!.body as BindingOptions;
  world.resources.report(id, false);
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['start-options', ACME.fixLogin, '']; cache.setQueryData(key, answer());
  world.resources.report(id, true); const firstAnswer = answer();
  let finish!: (value: BindingOptions) => void;
  const first = new Promise<BindingOptions>((resolve) => { finish = resolve; });
  const read = vi.fn<() => Promise<BindingOptions>>().mockReturnValueOnce(first).mockImplementation(() => Promise.resolve(answer()));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    const row = world.resources.inventoryView(person, 'agent', id);
    const old = applyChanges(resourceChanges('agent', id, { row }, 'news'), cache);
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    client.session.setState({ readerVersion: (previous ?? 0) + 1 });
    world.resources.report(id, false);
    const current = applyChanges(resourceChanges('agent', id, { row: world.resources.inventoryView(person, 'agent', id) }, 'news'), cache);
    finish(firstAnswer); await Promise.all([old, current]);
    expect(read).toHaveBeenCalledTimes(2); expect(cache.getQueryData(key)).toEqual(answer());
  } finally { finish(firstAnswer); stop(); cache.clear(); client.session.setState({ readerVersion: previous }); }
});
