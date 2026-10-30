/** One level of the reader's complete hierarchy, or name matches at every level, with keyset pages. */
import { infiniteQueryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import type { EverythingDirection, EverythingKind, EverythingRowPage, EverythingScope, EverythingSort, ProviderListScope } from '@/generated/api-types';

/** Kind-specific predicates belong to the shared server pager, rather than a loaded browser slice. */
export interface ResourceFilter {
  state?: string;
  kind?: string;
  harness?: string;
  view?: ProviderListScope;
  home?: string;
  details?: boolean;
}

export interface EverythingFilter {
  recent?: 'opened' | 'active';
  navigation?: boolean;
  showArchived?: boolean;
  hideDone?: boolean;
  keep?: string;
  scope: EverythingScope;
  kinds: readonly EverythingKind[];
  q: string;
  owner: string | null;
  createdFrom: string;
  createdTo: string;
  activeFrom?: string;
  sort: EverythingSort;
  direction: EverythingDirection;
  participating?: boolean;
  adminAccess?: boolean;
  resource?: ResourceFilter;
}

/** The contract's usual page keeps expanding a branch inexpensive. */
export const EVERYTHING_PAGE = 50;
/** Four requests keep an explicit Expand all useful without filling a small server pool. */
const OPEN_BRANCHES = 4;
let active = 0;
const waiting: (() => void)[] = [];

async function inTurn<T>(signal: AbortSignal | undefined, read: () => Promise<T>): Promise<T> {
  if (active >= OPEN_BRANCHES) await new Promise<void>((resolve) => waiting.push(resolve));
  else active += 1;
  try {
    signal?.throwIfAborted();
    return await read();
  } finally {
    const next = waiting.shift();
    if (next === undefined) active -= 1;
    else next();
  }
}

/** Only name search crosses every level. Other filters keep the tree's branches available. */
export const everythingQuery = (reader: string, parent: string | null, filter: EverythingFilter, cacheRoot = 'everything') => infiniteQueryOptions({
  // A resource list retains its mutation invalidation root while sharing this request and pager.
  queryKey: [cacheRoot, reader, parent, filter] as const,
  initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<EverythingRowPage> => inTurn(signal, () => ok(api.GET('/everything', {
    params: { query: {
      scope: filter.scope,
      ...(filter.recent === undefined ? {} : { recent: filter.recent }),
      ...(filter.navigation === undefined ? {} : { navigation: filter.navigation }),
      ...(filter.showArchived === undefined ? {} : { show_archived: filter.showArchived }),
      ...(filter.hideDone === undefined ? {} : { hide_done: filter.hideDone }),
      ...(filter.keep === undefined ? {} : { keep: filter.keep }),
      ...(filter.participating === true ? { participating: true } : {}),
      ...(filter.adminAccess === undefined ? {} : { admin_access: filter.adminAccess }),
      ...(parent === null ? {} : { parent }),
      ...(filter.q.trim() === '' ? {} : { q: filter.q.trim() }),
      ...(filter.kinds.length === 0 ? {} : { kinds: [...filter.kinds].sort().join(',') }),
      ...(filter.owner === null ? {} : { owner: filter.owner }),
      ...(filter.createdFrom === '' ? {} : { created_from: `${filter.createdFrom}T00:00:00.000Z` }),
      ...(filter.createdTo === '' ? {} : { created_to: `${filter.createdTo}T23:59:59.999Z` }),
      ...(filter.activeFrom === undefined || filter.activeFrom === '' ? {} : { active_from: `${filter.activeFrom}T00:00:00.000Z` }),
      ...(filter.resource?.details === true ? { resource_details: true } : {}),
      ...(filter.resource?.state === undefined ? {} : { resource_state: filter.resource.state }),
      ...(filter.resource?.kind === undefined ? {} : { resource_kind: filter.resource.kind }),
      ...(filter.resource?.harness === undefined ? {} : { harness: filter.resource.harness }),
      ...(filter.resource?.view === undefined ? {} : { resource_view: filter.resource.view }),
      ...(filter.resource?.home === undefined ? {} : { home: filter.resource.home }),
      sort: filter.sort, direction: filter.direction, limit: EVERYTHING_PAGE,
      ...(pageParam === null ? {} : { cursor: pageParam }),
    } }, signal,
  }))),
  getNextPageParam: (page) => page.next_cursor,
});
