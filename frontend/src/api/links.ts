/** The Links section's reads, and the one act that chooses which of its tabs a document shows.
 *
 * Every list is read a page at a time with the cursor the server sealed for this reader; nothing here counts, caches
 * or keeps what a page left out, so what the reader may not read is never here to show.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import type { DocumentLinks, LinkDirection, LinkGraph, LinksTab, RelatedItems } from '@/generated/api-types';

export type { DocumentLinks, LinkGraph, LinkedThing, LinkOccurrence, LinksTab } from '@/generated/api-types';

/** The tabs in the order the section shows them. */
export const LINKS_TABS: readonly LinksTab[] = ['referenced_by', 'references', 'related', 'graph'];

/** How fresh a list of Links is taken to be without asking again: other documents change it, and no feed says so. */
const LINKS_STALE_MS = 30_000;

export const linksKey = (document: string) => ['document-links', document] as const;

/** Which tabs a document shows, and whether the caller may change them. */
export const documentLinksQuery = (document: string) =>
  queryOptions({
    queryKey: [...linksKey(document), 'tabs'] as const,
    queryFn: ({ signal }): Promise<DocumentLinks> => ok(api.GET('/documents/{id}/links', { params: { path: { id: document } }, signal })),
    staleTime: LINKS_STALE_MS,
    refetchOnWindowFocus: false,
  });

/** Chooses the tabs of one document, or `null` to follow the place's setting again. */
export const setDocumentLinks = (document: string, shown: LinksTab[] | null): Promise<DocumentLinks> =>
  ok(api.PUT('/documents/{id}/links', { params: { path: { id: document } }, body: { shown } }));

/** What refers to a document, or what it refers to: sources or targets, page by page. */
export const referencesQuery = (document: string, direction: LinkDirection) =>
  infiniteQueryOptions({
    queryKey: [...linksKey(document), 'references', direction] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      ok(api.GET('/documents/{id}/references', {
        params: { path: { id: document }, query: { direction, ...(pageParam === null ? {} : { cursor: pageParam }) } },
        signal,
      })),
    getNextPageParam: (page) => page.next_cursor,
    staleTime: LINKS_STALE_MS,
    refetchOnWindowFocus: false,
    retry: false,
  });

/** The mentions of one source that follow those a row already shows, from the cursor the row carries. */
export const mentionsQuery = (document: string, of: string, first: string) =>
  infiniteQueryOptions({
    queryKey: [...linksKey(document), 'mentions', of] as const,
    initialPageParam: first,
    queryFn: ({ pageParam, signal }) =>
      ok(api.GET('/documents/{id}/references', { params: { path: { id: document }, query: { direction: 'in', of, cursor: pageParam } }, signal })),
    getNextPageParam: (page) => page.next_cursor,
    staleTime: LINKS_STALE_MS,
    refetchOnWindowFocus: false,
    retry: false,
  });

/** What a graph is asked for. */
export interface GraphOptions {
  depth: 1 | 2 | 3;
  kinds: readonly ('document' | 'chat' | 'thread')[];
  meaning: boolean;
  /** A node to centre on instead of the document, as `kind:id`. */
  center: string | null;
}

export const graphQuery = (document: string, options: GraphOptions) =>
  queryOptions({
    queryKey: [...linksKey(document), 'graph', options.depth, [...options.kinds].sort().join(','), options.meaning, options.center] as const,
    queryFn: ({ signal }): Promise<LinkGraph> =>
      ok(api.GET('/documents/{id}/graph', {
        params: { path: { id: document }, query: {
          depth: options.depth,
          meaning: options.meaning,
          ...(options.kinds.length === 0 || options.kinds.length === 3 ? {} : { kinds: options.kinds.join(',') }),
          ...(options.center === null ? {} : { center: options.center }),
        } },
        signal,
      })),
    staleTime: LINKS_STALE_MS,
    refetchOnWindowFocus: false,
    retry: false,
  });

/** The related items of a document, a page at a time; a revision of the document reuses its answer until it changes. */
export const relatedPagesQuery = (document: string, seq: number) =>
  infiniteQueryOptions({
    queryKey: ['document-related', document, seq, 'pages'] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<RelatedItems> =>
      ok(api.GET('/documents/{id}/related', {
        params: { path: { id: document }, query: pageParam === null ? {} : { cursor: pageParam } },
        signal,
      })),
    getNextPageParam: (page) => page.next_cursor ?? null,
    // An unfinished index can have completed before the section is opened again with the same revision.
    staleTime: (query) => query.state.data?.pages[0]?.meaning === 'not_ready' ? 0 : Infinity,
    refetchOnWindowFocus: false,
    retry: false,
  });
