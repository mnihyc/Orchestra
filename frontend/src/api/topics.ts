/** A topic's native environment selector includes placed and thread-bound environments after admission. */
import { collectionQuery } from '@/api/collection';
import { api, ok } from '@/api/client';
export const topicEnvironmentsPageQuery = (id: string, q: string, version: string | null = null) => collectionQuery(['topic-environments', id, version], q,
  async (query, signal) => {
    const page = await ok(api.GET('/topics/{id}/environments', { params: { path: { id }, query }, signal }));
    return { ...page, items: page.items.map(({ id, name, address, placement, status, owner, ports_exposed }) => ({ id, name, address, placement, status, owner, ports_exposed })) };
  });
