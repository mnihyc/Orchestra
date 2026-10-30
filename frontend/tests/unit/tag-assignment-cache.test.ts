/** Assigning tags updates the returned place and leaves the available tag definitions intact. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import type { Node, Tag } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

for (const [kind, id] of [['org', ACME.org], ['project', ACME.web], ['topic', ACME.fixLogin]] as const) {
  it(`applies the accepted ${kind} tags without putting a place into the tag catalogue or refetching either view`, async () => {
    const world = new World();
    const person = world.people.get('ada')!;
    const before = world.places.node(person, id);
    const catalogue = world.places.handle(person, 'GET', `/nodes/${id}/tags`, new URLSearchParams(), {})?.body as Tag[];
    const urgent = catalogue.find((tag) => tag.name === 'urgent');
    if (urgent === undefined) throw new Error('Missing tag fixture');
    const input = { tag_ids: [...new Set([...before.tags.map((tag) => tag.id), urgent.id])], expected_revision: before.revision };
    const answer = world.places.handle(person, 'PUT', `/nodes/${id}/tags`, new URLSearchParams(), input)?.body as Node;
    const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
    const reads = { node: vi.fn(() => Promise.resolve(before)), tags: vi.fn(() => Promise.resolve(catalogue)) };
    cache.setQueryData(['node', id], before);
    cache.setQueryData(['tags', id], catalogue);
    const stopNode = new QueryObserver(cache, { queryKey: ['node', id], queryFn: reads.node }).subscribe(() => undefined);
    const stopTags = new QueryObserver(cache, { queryKey: ['tags', id], queryFn: reads.tags }).subscribe(() => undefined);
    try {
      const accepted = findAct('PUT', `/nodes/${id}/tags`, { answer, input });
      if (accepted === null) throw new Error('Missing tag assignment act');
      await applyChanges(accepted.changes, cache);
      expect(cache.getQueryData<Node>(['node', id])?.tags.map((tag) => tag.id)).toEqual(input.tag_ids);
      expect(cache.getQueryData(['tags', id])).toEqual(catalogue);
      expect(reads.node).not.toHaveBeenCalled();
      expect(reads.tags).not.toHaveBeenCalled();
    } finally { stopNode(); stopTags(); cache.clear(); }
  });
}
