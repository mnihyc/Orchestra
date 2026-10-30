/** What the screens of places read, asked for again: kept for the callers that still ask after an act of their own; the registry of acts does it for every act.
 *
 * Every act of the application states in the registry of acts (`src/api/acts.ts`) what it changes, and the
 * registry asks for those views again before the act's caller goes on; what other people change arrives by the
 * live tree, which the registry reads row by row (`treeChanges`). Nothing else needs to ask. A caller that
 * still calls `invalidatePlaces` after its act asks a second time for what is already fresh; it is to be
 * dropped from each such caller as its screen is next changed.
 */
import type { QueryClient } from '@tanstack/react-query';

import { applyChanges } from '@/api/acts';

/** The kinds of thing screens of places read, by the first part of their keys. */
const PLACE_QUERIES = ['org', 'orgs', 'node', 'children', 'topics', 'activity', 'tags', 'deleted', 'members', 'invitations', 'roles'] as const;

/**
 * Marks what places read as out of date and asks for what is in view again, and brings the trees held up to date.
 * Resolves when the queries in view have their answers. What was read of the places in `gone`, which were deleted,
 * is not asked again: there is nothing to answer.
 */
export function invalidatePlaces(queryClient: QueryClient, _rootOrgId?: string | null, gone: readonly string[] = []): Promise<void> {
  return applyChanges(
    [
      ...PLACE_QUERIES.map((head) => ({ key: [head] })),
      ...gone.flatMap((id) => PLACE_QUERIES.map((head) => ({ key: [head], id, gone: true as const }))),
      { feed: 'trees' as const },
    ],
    queryClient,
  );
}
