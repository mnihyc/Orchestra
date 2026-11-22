/** Shown views consult the same first shell-bootstrap query that owns their initial reads. */
import type { QueryCache } from '@tanstack/react-query';

interface BootstrapRead { readonly state: { readonly data: unknown } }
interface BootstrapIndex { readonly first: BootstrapRead | undefined }
const indexes = new WeakMap<QueryCache, BootstrapIndex>();

export function shellBootstrapIndex(cache: QueryCache): BootstrapIndex {
  const existing = indexes.get(cache);
  if (existing !== undefined) return existing;

  // Alternate clients can already contain answers when a view first uses their cache.
  const matches = new Set<BootstrapRead>(cache.getAll().filter((query) => query.queryKey[0] === 'shell-bootstrap'));
  const index = { first: matches.values().next().value };
  cache.subscribe((event) => {
    if (event.type !== 'added' && event.type !== 'removed') return;
    const query = event.query;
    const key: unknown = query.queryKey;
    if (!Array.isArray(key) || key[0] !== 'shell-bootstrap') return;
    if (event.type === 'added') {
      if (cache.get(query.queryHash) !== query) return;
      matches.add(query);
      index.first ??= query;
    } else {
      matches.delete(query);
      if (index.first === query) index.first = matches.values().next().value;
    }
  });
  indexes.set(cache, index);
  return index;
}
