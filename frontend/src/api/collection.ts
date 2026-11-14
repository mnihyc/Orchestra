/** A complete metadata consumer follows native pages under the same requesting reader. */
import { client } from '@/api/client';
import { infiniteQueryOptions, type QueryClient, type QueryKey } from '@tanstack/react-query';

export interface PageOf<T> { items: T[]; next_cursor: string | null }

/** Only native list envelopes are traversed; detail and permission objects are never searched for identifiers. */
export function collectionRows(value: unknown): unknown[] {
  if (Array.isArray(value)) return value as unknown[];
  if (value === null || typeof value !== 'object') return [];
  if ('items' in value && Array.isArray(value.items)) return value.items as unknown[];
  if ('pages' in value && Array.isArray(value.pages)) return (value.pages as unknown[]).flatMap((page) =>
    page !== null && typeof page === 'object' && 'items' in page && Array.isArray(page.items) ? page.items as unknown[] : []);
  return [];
}

export interface CollectionInput { q: string; limit: number; cursor?: string }
export function collectionQuery<T>(key: QueryKey, q: string, read: (query: CollectionInput, signal: AbortSignal) => Promise<PageOf<T>>) {
  return infiniteQueryOptions({
    queryKey: [...key, 'pages', q] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => read({ q, limit: 50, ...(pageParam === null ? {} : { cursor: pageParam }) }, signal),
    getNextPageParam: (last) => last.next_cursor,
  });
}

/** A complete native acknowledgement replaces related searched pages without another request. */
export async function seedNamedPages<T>(cache: QueryClient, key: QueryKey, rows: T[], text: (row: T) => string): Promise<void> {
  const before = client.session.getState();
  for (const query of cache.getQueryCache().findAll({ queryKey: [...key, 'pages'] })) {
    await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
    if (client.session.getState().readerVersion !== before.readerVersion || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query) return;
    const search: unknown = query.queryKey.at(-1);
    if (typeof search !== 'string') continue;
    const words = search.toLowerCase().split(/\s+/u).filter(Boolean);
    const items = rows.filter((row) => words.every((word) => text(row).toLowerCase().includes(word)));
    cache.setQueryData(query.queryKey, { pages: [{ items, next_cursor: null }], pageParams: [null] });
  }
}

export async function readAllPages<T>(read: (cursor: string | null) => Promise<PageOf<T>>, signal: AbortSignal): Promise<T[]> {
  const before = client.session.getState(), rows: T[] = [], seen = new Set<string>();
  const current = () => {
    signal.throwIfAborted();
    const now = client.session.getState();
    if (now.readerVersion !== before.readerVersion || now.userId !== before.userId) throw new DOMException('The requesting reader changed.', 'AbortError');
  };
  let cursor: string | null = null;
  do {
    current();
    const page = await read(cursor);
    current();
    rows.push(...page.items);
    cursor = page.next_cursor;
    if (cursor !== null && seen.has(cursor)) throw new Error('The collection repeated a cursor.');
    if (cursor !== null) seen.add(cursor);
  } while (cursor !== null);
  return rows;
}
