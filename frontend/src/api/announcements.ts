/** One place-level announcement path for orgs, projects and topics. */
import { type InfiniteData, infiniteQueryOptions, type QueryClient } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { AnnouncementCreateBody, AnnouncementPage, PlaceAnnouncementCreateReply } from '@/generated/api-types';

/** A page matches the topic feed's twenty-row first load. */
const PAGE_SIZE = 20;

export const announcementsQuery = (place: string) => infiniteQueryOptions({
  queryKey: ['announcements', place], initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<AnnouncementPage> => ok(api.GET('/nodes/{id}/announcements', {
    params: { path: { id: place }, query: { limit: PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal,
  })),
  getNextPageParam: (page) => page.next_cursor,
});

export function announce(place: string, body: AnnouncementCreateBody): Promise<PlaceAnnouncementCreateReply> {
  return ok(api.POST('/nodes/{id}/announcements', { params: { path: { id: place } }, body }));
}

/** The accepted rows update the displayed history without another HTTP read. */
export async function applyAnnouncements(cache: QueryClient, place: string, reply: PlaceAnnouncementCreateReply): Promise<void> {
  const key = announcementsQuery(place).queryKey;
  const owner = cache.getQueryCache().find({ queryKey: key, exact: true });
  if (owner === undefined) return;
  // Keep the first page's older history if its opening read was still in flight when Send finished.
  // The query's identity also prevents a late answer from entering another reader's cache.
  if (owner.state.fetchStatus === 'fetching') {
    try { await owner.promise; } catch { return; }
  }
  if (cache.getQueryCache().find({ queryKey: key, exact: true }) !== owner) return;
  const received = new Set(reply.announcements.map((row) => row.id));
  cache.setQueryData<InfiniteData<AnnouncementPage, string | null>>(key, (before) => {
    const head = before?.pages[0];
    if (before === undefined || head === undefined) return before;
    const items = [...reply.announcements, ...head.items.filter((row) => !received.has(row.id))]
      .sort((one, other) => other.created_at.localeCompare(one.created_at) || other.id.localeCompare(one.id));
    return { ...before, pages: [{ ...head, items }, ...before.pages.slice(1).map((page) => ({ ...page, items: page.items.filter((row) => !received.has(row.id)) }))] };
  });
}
