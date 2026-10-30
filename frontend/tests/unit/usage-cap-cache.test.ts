/** Cap definitions omit derived use and eligibility, so each shown affected owner retakes its native status. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { CapInput, CapStatus, UsageCap } from '@/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { World } from '../doubles/world';

beforeEach(() => { vi.useFakeTimers({ toFake: ['Date'] }); vi.setSystemTime(new Date('2026-10-29T12:00:00Z')); });
afterEach(() => { vi.useRealTimers(); });

const cacheOf = () => new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
const capsKey = (id: string, covering = true) => ['usage', 'caps', `node:${id}:${String(covering)}`];
const input = (node: string): CapInput => ({ node_id: node, subject: { kind: 'everyone' }, split: 'shared', meter: 'tokens', period: 'day', amount: '1' });

for (const shown of [true, false]) it(`updates provider usage and its ${shown ? 'shown' : 'hidden'} model editor from the native cap status`, async () => {
  const { world, person } = fixture();
  const provider = RUNS_ON.openAiKey;
  const modelKey = ['provider-caps', provider], usageKey = ['usage', 'caps', `provider:${provider}`], otherKey = ['provider-caps', RUNS_ON.codexSubscription];
  const cache = cacheOf();
  const list = () => world.usage.handle(person, 'GET', '/usage/caps', new URLSearchParams({ provider_id: provider }), {})!.body as CapStatus[];
  const readModels = vi.fn(() => Promise.resolve(list())), readUsage = vi.fn(() => Promise.resolve(list())), readOther = vi.fn(() => Promise.resolve([]));
  for (const key of [modelKey, usageKey, otherKey]) cache.setQueryData(key, []);
  const stopModels = shown ? new QueryObserver(cache, { queryKey: modelKey, queryFn: readModels }).subscribe(() => undefined) : () => undefined;
  const stopUsage = new QueryObserver(cache, { queryKey: usageKey, queryFn: readUsage }).subscribe(() => undefined);
  const stopOther = new QueryObserver(cache, { queryKey: otherKey, queryFn: readOther }).subscribe(() => undefined);
  try {
    const accepted = world.usage.handle(person, 'POST', '/usage/caps', new URLSearchParams(), {
      provider_id: provider, subject: { kind: 'everyone' }, split: 'shared', meter: 'tokens', period: 'day', amount: '5000000',
    })!.body as UsageCap;
    await applyChanges(findAct('POST', '/usage/caps', { answer: accepted })!.changes, cache);
    expect(cache.getQueryData(usageKey)).toEqual(list());
    expect(cache.getQueryData(modelKey)).toEqual(shown ? list() : undefined);
    expect(readUsage).toHaveBeenCalledTimes(1);
    expect(readModels).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(readOther).not.toHaveBeenCalled();
  } finally { stopModels(); stopUsage(); stopOther(); cache.clear(); }
});
function fixture() {
  const world = new World();
  const person = world.people.get('ada')!;
  const list = (id: string) => world.usage.handle(person, 'GET', '/usage/caps', new URLSearchParams({ node_id: id, covering: 'true' }), {})!.body as CapStatus[];
  const make = (node: string) => world.usage.handle(person, 'POST', '/usage/caps', new URLSearchParams(), input(node))!.body as UsageCap;
  return { world, person, list, make };
}

it('shows the authoritative use of a newly created cap, expires hidden owners and leaves another place unread', async () => {
  const { list, make } = fixture();
  const cache = cacheOf();
  const owner = capsKey(ACME.web), hidden = capsKey(ACME.web, false), other = capsKey(ACME.infra);
  cache.setQueryData(other, []);
  cache.setQueryData(owner, []);
  cache.setQueryData(hidden, []);
  const read = vi.fn(() => Promise.resolve(list(ACME.web)));
  const readOther = vi.fn(() => Promise.resolve([]));
  const stopOther = new QueryObserver(cache, { queryKey: other, queryFn: readOther }).subscribe(() => undefined);
  const stop = new QueryObserver(cache, { queryKey: owner, queryFn: read }).subscribe(() => undefined);
  try {
    const accepted = make(ACME.web);
    await applyChanges(findAct('POST', '/usage/caps', { input: input(ACME.web), answer: accepted })!.changes, cache);
    expect(cache.getQueryData(owner)).toEqual(list(ACME.web));
    expect((cache.getQueryData<CapStatus[]>(owner))?.find((status) => status.cap.id === accepted.id)?.used).not.toBe('0');
    expect(read).toHaveBeenCalledTimes(1);
    expect(readOther).not.toHaveBeenCalled();
    expect(cache.getQueryData(hidden)).toBeUndefined();
  } finally { stop(); stopOther(); cache.clear(); }
});

it('retakes an inherited cap after an amount change and does not guess its new reached state', async () => {
  const { world, person, list, make } = fixture();
  const cap = make(ACME.org);
  const cache = cacheOf();
  const child = capsKey(ACME.web), parent = capsKey(ACME.org);
  cache.setQueryData(child, list(ACME.web));
  cache.setQueryData(parent, list(ACME.org));
  const read = vi.fn(() => Promise.resolve(list(ACME.web)));
  const stop = new QueryObserver(cache, { queryKey: child, queryFn: read }).subscribe(() => undefined);
  try {
    const accepted = world.usage.handle(person, 'PATCH', `/usage/caps/${cap.id}`, new URLSearchParams(), { amount: '1000000000', expected_revision: cap.revision })!.body as UsageCap;
    await applyChanges(findAct('PATCH', `/usage/caps/${cap.id}`, { answer: accepted })!.changes, cache);
    expect(cache.getQueryData(child)).toEqual(list(ACME.web));
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(parent)).toBeUndefined();
  } finally { stop(); cache.clear(); }
});

it('retakes the known cap owner after a bodyless delete without reading another place', async () => {
  const { world, person, list, make } = fixture();
  const cap = make(ACME.web);
  const cache = cacheOf();
  const owner = capsKey(ACME.web), other = capsKey(ACME.infra);
  cache.setQueryData(other, []);
  cache.setQueryData(owner, list(ACME.web));
  const read = vi.fn(() => Promise.resolve(list(ACME.web)));
  const readOther = vi.fn(() => Promise.resolve([]));
  const stopOther = new QueryObserver(cache, { queryKey: other, queryFn: readOther }).subscribe(() => undefined);
  const stop = new QueryObserver(cache, { queryKey: owner, queryFn: read }).subscribe(() => undefined);
  try {
    world.usage.handle(person, 'DELETE', `/usage/caps/${cap.id}`, new URLSearchParams(), {});
    await applyChanges(findAct('DELETE', `/usage/caps/${cap.id}`)!.changes, cache);
    expect(cache.getQueryData(owner)).toEqual(list(ACME.web));
    expect(cache.getQueryData<CapStatus[]>(owner)?.map((status) => status.cap.id)).not.toContain(cap.id);
    expect(read).toHaveBeenCalledTimes(1);
    expect(readOther).not.toHaveBeenCalled();
  } finally { stop(); stopOther(); cache.clear(); }
});

it('retires an older pending status read before publishing the accepted cap', async () => {
  const { list, make } = fixture();
  const cache = cacheOf();
  const owner = capsKey(ACME.web);
  const before = list(ACME.web);
  let finish!: (value: CapStatus[]) => void;
  const old = new Promise<CapStatus[]>((resolve) => { finish = resolve; });
  let signal: AbortSignal | undefined;
  const read = vi.fn(({ signal: given }: { signal: AbortSignal }) => {
    if (signal === undefined) { signal = given; return old; }
    return Promise.resolve(list(ACME.web));
  });
  cache.setQueryData(owner, before);
  const stop = new QueryObserver(cache, { queryKey: owner, queryFn: read, staleTime: 0 }).subscribe(() => undefined);
  try {
    const cap = make(ACME.web);
    await applyChanges(findAct('POST', '/usage/caps', { input: input(ACME.web), answer: cap })!.changes, cache);
    expect(signal?.aborted).toBe(true);
    expect(cache.getQueryData(owner)).toEqual(list(ACME.web));
    finish(before); await old;
    expect(cache.getQueryData(owner)).toEqual(list(ACME.web));
    expect(read).toHaveBeenCalledTimes(2);
  } finally { finish(before); stop(); cache.clear(); }
});
