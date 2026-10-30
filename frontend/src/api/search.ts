/** API client for Authorized search and the finder's names.
 *
 * Two requests. The names: whatever the caller may see that is called so, found from a few letters, and
 * with no letters the places they went to lately. The search: what was written, in the conversations of
 * threads, in chats and in documents, newest first, a page at a time. A system that does not answer
 * either says so with a 404, and the screen then has nothing to show of it.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import { readInThread, receiveThreadRead, threadQueryRead, threadReadSelector } from '@/api/threadRead';
import { stampThreadReceipt, threadReadChanged, type ThreadReadTarget } from '@/realtime/threadScope';
import type { NameHit, NameKind, NameSearchPage, RelatedItems, SearchHit, SearchHitPage, SearchSort, SettingDef, SearchStreamEvent } from '@/generated/api-types';
import { loadSearchCatalogues } from '@/i18n';

export type { NameHit, NameKind, SearchHit };

/** Related reads no body or preview; a document revision reuses its one ranked answer until it changes. */
export const relatedQuery = (documentId: string, seq: number) => infiniteQueryOptions({
  queryKey: ['document-related', documentId, seq] as const,
  initialPageParam: null as string | null,
  queryFn: async ({ pageParam, signal }): Promise<RelatedItems> => ok(api.GET('/documents/{id}/related', {
    params: { path: { id: documentId }, query: { ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal,
  })),
  getNextPageParam: (page) => page.next_cursor ?? undefined,
  // Only opening or an explicit next-page act reads; unfinished indexing may have completed since closing.
  staleTime: (query) => query.state.data?.pages[0]?.meaning === 'not_ready' ? 0 : Infinity,
  refetchOnWindowFocus: false,
  retry: false,
});

export const recentsKey = ['recents'] as const;

/** How many recents the left bar lists: a short list is a shortcut, a long one is a second tree. */
export const RECENTS_SHOWN = 5;

/** The orgs, projects, topics and threads the caller went to last. */
export const recentsQuery = queryOptions({
  queryKey: recentsKey,
  queryFn: async ({ signal }): Promise<NameHit[]> =>
    (await ok(api.GET('/search/names', { params: { query: { kinds: 'org,project,topic,thread', limit: RECENTS_SHOWN * 2 } }, signal }))).items,
  // A system that does not serve it yet is asked again only when the page is loaded anew.
  retry: false,
  retryOnMount: false,
  staleTime: 60_000,
});

// ---------------------------------------------------------------- names

/** The fewest and the most names the server answers with, as the contract states them. */
export const NAME_LIMIT_DEFAULT = 20;
export const NAME_LIMIT_MAX = 50;

/** Places, and the things written beside them: what "Go to" lists. */
export const PLACE_KINDS: readonly NameKind[] = ['org', 'project', 'topic', 'thread', 'document', 'document_container', 'chat', 'tag'];
/** People and groups of people. */
export const PEOPLE_KINDS: readonly NameKind[] = ['user', 'group'];
/** Everything a thread runs on, or that is shared with it. */
export const RESOURCE_KINDS: readonly NameKind[] = ['provider', 'agent', 'environment', 'host', 'template', 'mcp_server', 'mount', 'machine', 'skill'];
/** Every addressed kind shares Starred and Recent. */
export const ALL_NAME_KINDS: readonly NameKind[] = [...PLACE_KINDS, ...PEOPLE_KINDS, ...RESOURCE_KINDS];

export interface NameSearch {
  starred?: boolean;
  /** What was typed; empty asks for the caller's recents. */
  q: string;
  kinds: readonly NameKind[];
  limit?: number;
  /** Keeps to one tree of orgs. */
  orgId?: string;
  participating?: boolean;
}

/** How long a list of names counts as fresh: a person typing and deleting letters comes back to the same answers. */
const NAMES_FRESH_MS = 15_000;

export const namesKey = (search: NameSearch) => ['names', search.kinds.join(','), search.q, search.limit ?? NAME_LIMIT_DEFAULT, search.orgId ?? '', search.starred ?? false, search.participating ?? false] as const;

/** The names that match, or with no letters the caller's recents. The answer to the letters before stays up until the next arrives. */
export const namesQuery = (search: NameSearch) =>
  queryOptions({
    queryKey: namesKey(search),
    queryFn: async ({ signal }): Promise<NameSearchPage> => {
      const length = Array.from(search.q.trim()).length;
      if (length > 0 && length < NAME_MIN_CHARS) return { items: [], partial: false };
      return ok(
        api.GET('/search/names', {
          params: {
            query: {
              ...(search.q === '' ? {} : { q: search.q }),
              kinds: search.kinds.join(','),
              limit: search.limit ?? NAME_LIMIT_DEFAULT,
              ...(search.orgId === undefined ? {} : { org_id: search.orgId }),
              ...(search.participating === true ? { participating: true } : {}),
              ...(search.starred === true ? { starred: true } : {}),
            },
          },
          signal,
        }),
      );
    },
    select: (page: NameSearchPage): NameHit[] => page.items,
    staleTime: NAMES_FRESH_MS,
    // A lookup made while typing is not made again by itself: the next letter asks anew.
    retry: false,
    placeholderData: (previous, held) => held?.queryKey.at(-1) === (search.participating ?? false) && held.queryKey.at(-2) === (search.starred ?? false) ? previous : undefined,
  });

// ---------------------------------------------------------------- the registry the finder reads settings from

/** Every setting there is, as the registry defines it: read once, since it changes only with a release. */
export const settingRegistryQuery = queryOptions({
  queryKey: ['settings-registry'] as const,
  queryFn: async ({ signal }): Promise<SettingDef[]> => {
    const [registry] = await Promise.all([ok(api.GET('/settings/registry', { signal })), loadSearchCatalogues()]);
    return registry;
  },
  staleTime: Number.POSITIVE_INFINITY,
  retry: false,
});

// ---------------------------------------------------------------- what was written

/** The fewest characters of a search of what was written, and of one that can use the index. */
export const SEARCH_MIN_CHARS = 2;
/** Names remain a two-character lookup. */
export const NAME_MIN_CHARS = 2;
export const SEARCH_INDEXED_MIN_CHARS = 3;
/** The most characters of the words around a match. */
export const SEARCH_SNIPPET_MAX_CHARS = 240;

/** The kinds of item of a conversation that a search can be kept to, besides chats and documents. */
export const ITEM_KINDS = ['user_message', 'agent_message', 'reasoning', 'plan', 'tool_call', 'approval', 'announcement', 'marker', 'error'] as const;
export type ItemKind = (typeof ITEM_KINDS)[number];
/** What a search can be kept to: an item kind, the messages of chats, or documents. */
export type SearchKind = ItemKind | 'chat_message' | 'document';
export const SEARCH_KINDS: readonly SearchKind[] = [...ITEM_KINDS, 'chat_message', 'document'];

/** How many hits a page holds: a screenful of cards, so that a page is read before the next is wanted. */
export const SEARCH_PAGE_SIZE = 20;

/** What a search is asked: the words, and what to keep to. */
export interface SearchFilters {
  regex?: boolean;
  sort?: SearchSort | undefined;
  participating?: boolean;
  chat_id?: string | undefined;
  document_id?: string | undefined;
  with_file?: boolean | undefined;
  mentions_me?: boolean | undefined;
  q: string;
  /** A node: only what lies in it and below it. */
  scope?: string | undefined;
  kinds?: readonly SearchKind[] | undefined;
  author?: string | undefined;
  created_by?: string | undefined;
  edited_by?: string | undefined;
  owner?: string | undefined;
  date_field?: 'created' | 'edited' | undefined;
  title_only?: boolean | undefined;
  shared_with_me?: boolean | undefined;
  file_type?: string | undefined;
  /** A moment, as the API writes it. */
  from?: string | undefined;
  to?: string | undefined;
}

/** Whether the words are long enough to ask for, and why not when they are not: the search says so before the server has to. */
export function tooShort(q: string, scope: { narrow: boolean; streamed?: boolean }): 'none' | 'short' | 'indexed' | null {
  const length = Array.from(q.trim()).length;
  if (length === 0) return 'none';
  if (length < SEARCH_MIN_CHARS && scope.streamed !== true) return 'short';
  if (length < SEARCH_INDEXED_MIN_CHARS && !scope.narrow && scope.streamed !== true) return 'indexed';
  return null;
}

export const searchKey = (filters: SearchFilters) =>
  ['search', filters.q.trim(), filters.scope ?? '', filters.kinds === undefined ? '*' : filters.kinds.join(','), filters.author ?? '', filters.created_by ?? '', filters.edited_by ?? '', filters.owner ?? '', filters.date_field ?? '', filters.title_only ?? false, filters.shared_with_me ?? false, filters.file_type ?? '', filters.from ?? '', filters.to ?? '', filters.chat_id ?? '', filters.document_id ?? '', filters.with_file ?? false, filters.mentions_me ?? false, filters.regex ?? false, filters.sort ?? 'newest', filters.participating ?? false] as const;

/** The hits of a search, newest first, a page at a time. */
export const searchQuery = (filters: SearchFilters, limit: number = SEARCH_PAGE_SIZE, historyThread?: ThreadReadTarget) => {
  const reading = historyThread === undefined ? undefined : threadQueryRead(historyThread, searchKey(filters));
  return infiniteQueryOptions({
    queryKey: reading?.key ?? searchKey(filters),
    enabled: reading?.enabled ?? true,
    initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<SearchHitPage> => {
      const captured = await reading?.capture(signal, pageParam !== null);
      if (captured !== undefined && filters.scope !== captured.threadId) throw threadReadChanged();
      // The request's own types turn the pairs of a match into lists; the contract's are pairs.
      const fetch = async (readSignal: AbortSignal) => (await ok(
        api.GET('/search', {
          params: {
            query: {
              q: filters.q.trim(),
              ...(captured === undefined ? {} : threadReadSelector(captured)),
              ...(filters.sort === undefined ? {} : { sort: filters.sort }),
              ...(filters.participating === true ? { participating: true } : {}),
              ...(filters.scope === undefined ? {} : { scope: filters.scope }),
              ...(filters.kinds === undefined ? {} : { kinds: filters.kinds.join(',') }),
              ...(filters.author === undefined ? {} : { author_id: filters.author }),
              ...(filters.created_by === undefined ? {} : { created_by_id: filters.created_by }),
              ...(filters.edited_by === undefined ? {} : { edited_by_id: filters.edited_by }),
              ...(filters.owner === undefined ? {} : { owner_id: filters.owner }),
              ...(filters.date_field === undefined ? {} : { date_field: filters.date_field }),
              ...(filters.title_only === true ? { title_only: true } : {}),
              ...(filters.shared_with_me === true ? { shared_with_me: true } : {}),
              ...(filters.file_type === undefined ? {} : { file_type: filters.file_type }),
              ...(filters.from === undefined ? {} : { from: filters.from }),
              ...(filters.to === undefined ? {} : { to: filters.to }),
              ...(filters.chat_id === undefined ? {} : { chat_id: filters.chat_id }),
              ...(filters.document_id === undefined ? {} : { document_id: filters.document_id }),
              ...(filters.with_file === true ? { with_file: true } : {}),
              ...(filters.mentions_me === true ? { mentions_me: true } : {}),
              limit,
              ...(pageParam === null ? {} : { cursor: pageParam }),
            },
          },
          signal: readSignal,
        }),
      )) as SearchHitPage;
      const page = captured === undefined ? await fetch(signal) : await readInThread(captured, fetch, signal);
      if (captured !== undefined) {
        for (const hit of page.items) if (hit.kind === 'item') receiveThreadRead(captured, hit.projection, signal);
        stampThreadReceipt(page, captured);
      }
      return page;
    },
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    staleTime: 15_000,
    // A refusal of the query is the answer; only the network is asked again, and the reader can say so.
    retry: false,
  });
};

/** How many hits the finder shows of a search before it leads to the page of the search. */
export const SEARCH_PREVIEW = 5;

/** The first few hits of a search, for the finder. */
export const searchPreviewQuery = (filters: SearchFilters) =>
  queryOptions({
    queryKey: ['search-preview', ...searchKey(filters)] as const,
    queryFn: async ({ signal }): Promise<SearchHitPage> =>
      (await ok(api.GET('/search', { params: { query: { q: filters.q.trim(), limit: SEARCH_PREVIEW, ...(filters.participating === true ? { participating: true } : {}), ...(filters.sort === undefined ? {} : { sort: filters.sort }) } }, signal }))) as SearchHitPage,
    staleTime: 15_000,
    retry: false,
    placeholderData: (previous, held) => held?.queryKey.at(-1) === (filters.participating ?? false) && held.queryKey.at(-2) === (filters.sort ?? 'newest') ? previous : undefined,
  });

/** One NDJSON response owns the two server-side retrievals; cancelling its signal cancels both. */
export async function streamSearch(filters: SearchFilters, receive: (event: SearchStreamEvent) => void, signal: AbortSignal, cursor?: string, immediate = true, typingAgeMs = 0): Promise<void> {
  const stream = await ok(api.GET('/search/stream', {params: {query: {
    q: filters.q.trim(), immediate, typing_age_ms: typingAgeMs,
    ...(filters.regex === true ? {regex: true} : {}),
    ...(filters.sort === undefined ? {} : {sort: filters.sort}),
    ...(filters.scope === undefined ? {} : {scope: filters.scope}),
    ...(filters.kinds === undefined ? {} : {kinds: filters.kinds.join(',')}),
    ...(filters.author === undefined ? {} : {author_id: filters.author}),
    ...(filters.created_by === undefined ? {} : {created_by_id: filters.created_by}),
    ...(filters.edited_by === undefined ? {} : {edited_by_id: filters.edited_by}),
    ...(filters.owner === undefined ? {} : {owner_id: filters.owner}),
    ...(filters.date_field === undefined ? {} : {date_field: filters.date_field}),
    ...(filters.title_only === true ? {title_only: true} : {}),
    ...(filters.shared_with_me === true ? {shared_with_me: true} : {}),
    ...(filters.file_type === undefined ? {} : {file_type: filters.file_type}),
    ...(filters.from === undefined ? {} : {from: filters.from}), ...(filters.to === undefined ? {} : {to: filters.to}),
    ...(filters.chat_id === undefined ? {} : {chat_id: filters.chat_id}), ...(filters.document_id === undefined ? {} : {document_id: filters.document_id}),
    ...(filters.with_file === true ? {with_file: true} : {}), ...(filters.mentions_me === true ? {mentions_me: true} : {}),
    ...(filters.participating === true ? {participating: true} : {}), ...(cursor === undefined ? {} : {cursor}),
  }}, signal, parseAs: 'stream'}));
  if (stream === null) throw new NetworkError('failed');
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let pending = '';
  const progress = {ended: false};
  const canceled = () => signal.aborted;
  const accept = (line: string) => {
    const event = JSON.parse(line) as SearchStreamEvent;
    if (event.type === 'cursor') progress.ended = true;
    receive(event);
  };
  try {
    while (!canceled()) {
      const {value, done} = await reader.read();
      pending += decoder.decode(value, {stream: !done});
      let newline: number;
      while ((newline = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, newline); pending = pending.slice(newline + 1);
        if (line.trim() !== '') accept(line);
      }
      if (done) {
        if (pending.trim() !== '') accept(pending);
        if (!progress.ended && !canceled()) throw new NetworkError('failed');
        break;
      }
    }
  } finally { await reader.cancel().catch(() => undefined); reader.releaseLock(); }
}
