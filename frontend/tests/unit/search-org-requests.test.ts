/** A rail preview sends the same real scope as its cache key; query changes retire old-org placeholders. */
import { LiveQueryClient } from '@/api/query';
import { isPeopleProjection } from '@/api/peopleChanges';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { nameByIdQuery, namesQuery, searchPreviewQuery } from '@/api/search';
import type { NameHit } from '@/generated/api-types';
import type { HttpTransport } from '../doubles/http-wire';
const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.get } }; });
afterEach(() => vi.clearAllMocks());

it('resolves a saved target by its exact kind and identity without depending on recents', async () => {
  const hit: NameHit = { id: 'target-topic', kind: 'topic', name: 'Release plan', home_path: [], path: [], address: null, avatar: null, archived: false, color: null, custom_color: null, owner: null, recent: false, tag: null, thread_attention: null, username: null };
  calls.get.mockResolvedValue({ data: { items: [hit], partial: false }, response: new Response(null, { status: 200 }) });
  const cache = new LiveQueryClient();
  try {
    expect(await cache.query(nameByIdQuery(hit.kind, hit.id))).toEqual(hit);
    const signal: unknown = expect.any(AbortSignal);
    expect(calls.get).toHaveBeenLastCalledWith('/search/names', expect.objectContaining({ params: { query: { id: hit.id, kinds: hit.kind } }, signal }));
    expect(await cache.query(nameByIdQuery('project', hit.id))).toBeNull();
    expect(nameByIdQuery(hit.kind, hit.id).queryKey).not.toEqual(nameByIdQuery('project', hit.id).queryKey);
    expect(isPeopleProjection(nameByIdQuery(hit.kind, hit.id).queryKey)).toBe(true);
  } finally { cache.clear(); }
});

it('sends root scope and leaves it out only on the explicit global preview', async () => {
  calls.get.mockResolvedValue({ data: { items: [], next_cursor: null }, response: new Response(null, { status: 200 }) });
  const cache = new LiveQueryClient();
  try {
    const scoped = searchPreviewQuery({ q: 'canary', scope: 'work' });
    await cache.query(scoped);
    expect(calls.get).toHaveBeenLastCalledWith('/search', expect.objectContaining({ params: { query: { q: 'canary', limit: 5, scope: 'work' } } }));
    await cache.query(searchPreviewQuery({ q: 'canary' }));
    expect(calls.get).toHaveBeenLastCalledWith('/search', expect.objectContaining({ params: { query: { q: 'canary', limit: 5 } } }));
    expect(scoped.placeholderData).toBeUndefined();
    expect(namesQuery({ q: 'canary', kinds: ['topic'], orgId: 'work' }).queryKey)
      .not.toEqual(namesQuery({ q: 'canary', kinds: ['topic'], orgId: 'personal' }).queryKey);
  } finally { cache.clear(); }
});
