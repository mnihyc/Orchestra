/** Accepted announcements join the existing history, including a late first page and older pages. */
import { InfiniteQueryObserver, QueryClient } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { announcementsQuery, applyAnnouncements } from '@/api/announcements';
import type { Announcement, AnnouncementPage, PlaceAnnouncementCreateReply } from '@/generated/api-types';
import { ACME, userRef } from '../doubles/fixtures';
import { World } from '../doubles/world';

function announcement(id: string, at: string): Announcement {
  const person = new World().people.get('ada');
  if (person === undefined) throw new Error('Missing account fixture');
  return { id, topic_id: ACME.fixLogin, group_id: 'group', category: 'notice', title: 'Release', body: 'The release is ready.',
    severity: 'info', from: { user: userRef(person.me) }, targets: 'all', files: [], delivery: 'steer', fallback: 'next_turn',
    wake_idle: false, delivered: { interrupted: 0, steered: 1, queued: 0, deferred: 0 }, created_at: at, topic_seq: 1 };
}
const older = announcement('older', '2026-10-01T10:00:00Z');
const newer = announcement('newer', '2026-10-01T11:00:00Z');
const reply: PlaceAnnouncementCreateReply = { group_id: 'group', announcements: [newer], topics_skipped: 0 };

it('shows an accepted announcement without refetching its visible history', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const read = vi.fn<() => Promise<AnnouncementPage>>().mockResolvedValue({ items: [older], next_cursor: null });
  const options = { ...announcementsQuery(ACME.fixLogin), queryFn: read, staleTime: Infinity };
  const observer = new InfiniteQueryObserver(cache, options);
  const stop = observer.subscribe(() => undefined);
  try {
    await cache.infiniteQuery(options);
    await applyAnnouncements(cache, ACME.fixLogin, reply);
    expect(observer.getCurrentResult().data?.pages.flatMap((page) => page.items.map((row) => row.id))).toEqual(['newer', 'older']);
    expect(read).toHaveBeenCalledTimes(1);
  } finally { stop(); cache.clear(); }
});

it('keeps older pages and their cursors while a repeated receipt occurs only once', async () => {
  const cache = new QueryClient();
  const key = announcementsQuery(ACME.fixLogin).queryKey;
  cache.setQueryData(key, { pages: [{ items: [older], next_cursor: 'older' }, { items: [newer], next_cursor: 'tail' }], pageParams: [null, 'older'] });
  await applyAnnouncements(cache, ACME.fixLogin, reply);
  await applyAnnouncements(cache, ACME.fixLogin, reply);
  expect(cache.getQueryData(key)).toEqual({ pages: [{ items: [newer, older], next_cursor: 'older' }, { items: [], next_cursor: 'tail' }], pageParams: [null, 'older'] });
  cache.clear();
});

it('joins the accepted rows after an already pending first page without losing its history', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  let complete!: (page: AnnouncementPage) => void;
  const page = new Promise<AnnouncementPage>((resolve) => { complete = resolve; });
  const read = vi.fn().mockReturnValue(page);
  const opening = cache.infiniteQuery({ ...announcementsQuery(ACME.fixLogin), queryFn: read });
  const accepted = applyAnnouncements(cache, ACME.fixLogin, reply);
  complete({ items: [older], next_cursor: 'older' });
  await opening; await accepted;
  expect(cache.getQueryData(announcementsQuery(ACME.fixLogin).queryKey)?.pages[0]).toEqual({ items: [newer, older], next_cursor: 'older' });
  expect(read).toHaveBeenCalledTimes(1);
  cache.clear();
});
