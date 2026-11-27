/** API client for Shortcuts: the pointers to documents, chats, threads and places that a place keeps, and the moving
 * and removing of one wherever it is kept.
 *
 * A person's own shortcuts are what they starred: they are read and kept through `personal.ts` (`/me/favorites`),
 * and are moved and removed here like any other. A shortcut names its target as its reader may see it now; the
 * target is never touched by anything here.
 */
import { infiniteQueryOptions } from '@tanstack/react-query';

import { api, done, ok } from '@/api/client';
import type { PersonalTarget, Shortcut } from '@/generated/api-types';

export type { Shortcut } from '@/generated/api-types';

export const shortcutsKey = (nodeId: string) => ['shortcuts', nodeId] as const;

/** How many shortcuts of a place a page asks for. */
export const SHORTCUTS_PAGE = 100;

/** The shortcuts kept in a place, in the order they were put there. */
export const shortcutsOfPlaceQuery = (nodeId: string) =>
  infiniteQueryOptions({
    queryKey: shortcutsKey(nodeId),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      ok(
        api.GET('/nodes/{id}/shortcuts', {
          params: { path: { id: nodeId }, query: { limit: SHORTCUTS_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) } },
          signal,
        }),
      ),
    getNextPageParam: (page) => page.next_cursor,
  });

/** Keeps a shortcut in a place; the one kept there already answers. */
export function keepShortcut(nodeId: string, target: PersonalTarget): Promise<Shortcut> {
  return ok(api.POST('/nodes/{id}/shortcuts', { params: { path: { id: nodeId } }, body: target }));
}

/** Puts a shortcut in another place, or with `null` in the person's own list. */
export function moveShortcut(id: string, nodeId: string | null): Promise<Shortcut> {
  return ok(api.POST('/shortcuts/{id}/move', { params: { path: { id } }, body: { node_id: nodeId } }));
}

/** Removes a shortcut; its target is not touched. */
export function removeShortcut(id: string): Promise<void> {
  return done(api.DELETE('/shortcuts/{id}', { params: { path: { id } } }));
}
