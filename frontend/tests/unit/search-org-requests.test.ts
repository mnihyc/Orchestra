/** A rail preview sends the same real scope as its cache key; query changes retire old-org placeholders. */
import { LiveQueryClient } from '@/api/query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { namesQuery, searchPreviewQuery } from '@/api/search';
import type { HttpTransport } from '../doubles/http-wire';
const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.get } }; });
afterEach(() => vi.clearAllMocks());
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
