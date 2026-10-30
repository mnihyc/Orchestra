/** API client for Favorites, recents, drafts and read state, and what the interface keeps for a person.
 *
 * So far: a person's favorites, in the order they put them, the visit that the client reports on
 * going to a node, and what the interface keeps for the person across devices (how much of the
 * agent's work is open in a conversation, how a difference is laid out). Favorites are the person's
 * own and easily undone, so a screen shows a change at once and puts it right if the server refuses.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, done, ok } from '@/api/client';
import type { Favorite, PersonalTarget } from '@/generated/api-types';

export const favoritesKey = ['favorites'] as const;

/** The nodes a person starred, in order. */
export const favoritesQuery = queryOptions({
  queryKey: favoritesKey,
  queryFn: ({ signal }): Promise<Favorite[]> => ok(api.GET('/me/favorites', { signal })),
});

/**
 * Stars a node, or puts one that is starred somewhere else in the list. Where it goes is `after`: the
 * node it follows, `null` for the very first place, and left out for the end.
 */
export function starNode(nodeId: string, after?: string | null): Promise<void> {
  return done(
    api.PUT('/me/favorites/{node_id}', {
      params: { path: { node_id: nodeId } },
      body: after === undefined ? {} : { after_node_id: after },
    }),
  );
}

export function unstarNode(nodeId: string): Promise<void> {
  return done(api.DELETE('/me/favorites/{node_id}', { params: { path: { node_id: nodeId } } }));
}

/** Tells the server that the person went to a node, which is what makes it one of their recents. */
export function recordVisit(nodeId: string): Promise<void> {
  return done(api.POST('/me/recents', { body: { node_id: nodeId } }));
}

/** Stars any readable kind/id through the same personal list as node favorites. */
export function starItem(target: PersonalTarget): Promise<void> {
  return done(api.PUT('/me/favorites/{kind}/{id}', { params: { path: target }, body: {} }));
}

/** Removes only that kind/id, including when its page is no longer readable. */
export function unstarItem(target: PersonalTarget): Promise<void> {
  return done(api.DELETE('/me/favorites/{kind}/{id}', { params: { path: target } }));
}

/** Records a server-timed visit and updates its age-decaying count. */
export function recordItemVisit(target: PersonalTarget): Promise<void> {
  return done(api.POST('/me/visits', { body: target }));
}

// ---------------------------------------------------------------- what the interface remembers

export const meStateKey = ['me-state'] as const;

/** Everything the interface keeps for the person, by key. */
export const meStateQuery = queryOptions({
  queryKey: meStateKey,
  queryFn: ({ signal }): Promise<Record<string, unknown>> => ok(api.GET('/me/state', { signal })),
});

/** Replaces the whole value of one key of that state. */
export function setMeState(key: string, value: unknown): Promise<void> {
  return done(api.PUT('/me/state/{key}', { params: { path: { key } }, body: value }));
}
