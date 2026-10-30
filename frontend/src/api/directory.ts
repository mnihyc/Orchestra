/** API client for Org membership with domains and moves, groups and service identities.
 *
 * What the members of an org screen asks: the people whose domain is that org or an org inside it, by
 * name and a page at a time, or only those whose domain is that org itself. A person's domain is the one
 * org of the tree they belong to; the role they hold there is the one the list shows.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import type {
  ApiTokenCreateBody,
  TokenCreated,
  ApiTokenInfo,
  Group,
  Holder,
  HolderInput,
  Holdings,
  Member,
  MemberMovePlan,
  MemberPage,
  OrgTokenPage,
  OutsidePersonPage,
  OutsideRemovalPlan,
  OutsideRemovalResult,
  OutsideSourcePage,
  ResourceFate,
  Role,
  ServiceAccount,
  Suspension,
  Tag,
  UserRef,
} from '@/generated/api-types';
import { PAGE_SIZE } from '@/api/nodes';

export const membersKey = (orgId: string) => ['members', orgId] as const;

export interface MemberFilters {
  /** Only those whose domain is the org itself. */
  direct: boolean;
  /** Only those whose name or username holds this. */
  q: string;
}

/** The members of an org at whatever level it lies, a page at a time. */
export const membersQuery = (orgId: string, filters: MemberFilters) =>
  infiniteQueryOptions({
    queryKey: [...membersKey(orgId), filters] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<MemberPage> =>
      ok(
        api.GET('/orgs/{id}/members', {
          params: {
            path: { id: orgId },
            query: {
              limit: PAGE_SIZE,
              ...(filters.direct ? { direct: true } : {}),
              ...(filters.q.trim() === '' ? {} : { q: filters.q.trim() }),
              ...(pageParam === null ? {} : { cursor: pageParam }),
            },
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

// ---------------------------------------------------------------- what is done to a member

export const suspensionsKey = (orgId: string) => ['suspensions', orgId] as const;

/** Everybody held out of an org now, with why and by whom. For whoever manages its members. */
export const suspensionsQuery = (orgId: string) =>
  queryOptions({
    queryKey: suspensionsKey(orgId),
    queryFn: ({ signal }): Promise<Suspension[]> => ok(api.GET('/orgs/{id}/suspensions', { params: { path: { id: orgId } }, signal })),
  });

/** Holds a member out of an org, and everything inside it, until it is lifted. What they own stays theirs. */
export function suspendMember(orgId: string, userId: string, reason: string): Promise<Suspension> {
  return ok(
    api.POST('/orgs/{id}/members/{user_id}/suspend', {
      params: { path: { id: orgId, user_id: userId } },
      body: reason.trim() === '' ? {} : { reason: reason.trim() },
    }),
  );
}

/** Lifts a suspension: everything is as it was. */
export function resumeMember(orgId: string, userId: string): Promise<void> {
  return done(api.POST('/orgs/{id}/members/{user_id}/resume', { params: { path: { id: orgId, user_id: userId } } }));
}

/** What changing a member's domain would do, said before anything does it. */
export function previewMemberMove(orgId: string, userId: string, to: string, role: Role | null, signal?: AbortSignal): Promise<MemberMovePlan> {
  return ok(
    api.POST('/orgs/{id}/members/{user_id}/move/preview', {
      params: { path: { id: orgId, user_id: userId } },
      body: { to, ...(role === null ? {} : { role }) },
      ...(signal ? { signal } : {}),
    }),
  );
}

/** Changes a member's domain; the role they held is taken from the old one and given on the new, in one step. */
export function moveMember(orgId: string, userId: string, to: string, role: Role | null): Promise<Member> {
  return ok(
    api.POST('/orgs/{id}/members/{user_id}/move', {
      params: { path: { id: orgId, user_id: userId } },
      body: { to, ...(role === null ? {} : { role }) },
    }),
  );
}

export const holdingsKey = (orgId: string, userId: string) => ['holdings', orgId, userId] as const;

/** Everything of a person's in a tree that ending their membership would touch. */
export const holdingsQuery = (orgId: string, userId: string) =>
  queryOptions({
    queryKey: holdingsKey(orgId, userId),
    queryFn: ({ signal }): Promise<Holdings> =>
      ok(api.GET('/orgs/{id}/members/{user_id}/holdings', { params: { path: { id: orgId, user_id: userId } }, signal })),
    // What a person owns changes while another screen is open; a dialog about it must not show an old answer.
    staleTime: 0,
  });

/** Ends the membership of the root org with everything it holds; `fates` say what becomes of each managed resource. */
export function removeMember(orgId: string, userId: string, fates: Readonly<Record<string, ResourceFate>>): Promise<void> {
  return done(
    api.DELETE('/orgs/{id}/members/{user_id}', {
      params: { path: { id: orgId, user_id: userId } },
      body: Object.keys(fates).length === 0 ? {} : { resources: { ...fates } },
    }),
  );
}

// ---------------------------------------------------------------- groups

export const groupsKey = (orgId: string) => ['groups', orgId] as const;

/** The groups an org keeps, and with `under` also those the orgs inside it keep. */
export const groupsQuery = (orgId: string, under: boolean) =>
  queryOptions({
    queryKey: [...groupsKey(orgId), under] as const,
    queryFn: ({ signal }): Promise<Group[]> =>
      ok(api.GET('/orgs/{id}/groups', { params: { path: { id: orgId }, query: under ? { under: true } : {} }, signal })),
  });

export function createGroup(orgId: string, body: { name: string; description?: string }, key: string): Promise<Group> {
  return ok(api.POST('/orgs/{id}/groups', { params: { path: { id: orgId } }, body, ...repeatable(key) }));
}

export function updateGroup(id: string, body: { name?: string; description?: string }): Promise<Group> {
  return ok(api.PATCH('/groups/{id}', { params: { path: { id } }, body }));
}

export function deleteGroup(id: string): Promise<void> {
  return done(api.DELETE('/groups/{id}', { params: { path: { id } } }));
}

export const groupMembersKey = (groupId: string) => ['group-members', groupId] as const;
export const groupHoldersKey = (groupId: string) => ['group-holders', groupId] as const;
export const groupHoldersQuery = (groupId: string) => queryOptions({ queryKey: groupHoldersKey(groupId),
  queryFn: ({ signal }): Promise<Holder[]> => ok(api.GET('/groups/{id}/holders', { params: { path: { id: groupId } }, signal })) });
export function addGroupHolder(groupId: string, body: HolderInput): Promise<void> {
  return done(api.PUT('/groups/{id}/holders', { params: { path: { id: groupId } }, body }));
}
export function removeGroupHolder(groupId: string, body: HolderInput): Promise<void> {
  return done(api.DELETE('/groups/{id}/holders', { params: { path: { id: groupId } }, body }));
}

export const groupMembersQuery = (groupId: string) =>
  queryOptions({
    queryKey: groupMembersKey(groupId),
    queryFn: ({ signal }): Promise<UserRef[]> => ok(api.GET('/groups/{id}/members', { params: { path: { id: groupId } }, signal })),
  });

export function addGroupMember(groupId: string, userId: string): Promise<void> {
  return done(api.PUT('/groups/{id}/members/{user_id}', { params: { path: { id: groupId, user_id: userId } } }));
}

export function removeGroupMember(groupId: string, userId: string): Promise<void> {
  return done(api.DELETE('/groups/{id}/members/{user_id}', { params: { path: { id: groupId, user_id: userId } } }));
}

// ---------------------------------------------------------------- tags

/** Makes a tag that topics here can carry. */
export function createTag(nodeId: string, body: { name: string; color: number }, key: string): Promise<Tag> {
  return ok(api.POST('/nodes/{id}/tags', { params: { path: { id: nodeId } }, body, ...repeatable(key) }));
}

export function updateTag(id: string, body: { name?: string; color?: number }): Promise<Tag> {
  return ok(api.PATCH('/tags/{id}', { params: { path: { id } }, body }));
}

/** Takes a tag away; it is removed from every topic that carried it. */
export function deleteTag(id: string): Promise<void> {
  return done(api.DELETE('/tags/{id}', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- service identities

export const serviceAccountsKey = (orgId: string) => ['service-accounts', orgId] as const;

export const serviceAccountsQuery = (orgId: string) =>
  queryOptions({
    queryKey: serviceAccountsKey(orgId),
    queryFn: ({ signal }): Promise<ServiceAccount[]> =>
      ok(api.GET('/orgs/{id}/service-accounts', { params: { path: { id: orgId } }, signal })),
  });

export function createServiceAccount(orgId: string, body: { username: string; display_name: string }, key: string): Promise<ServiceAccount> {
  return ok(api.POST('/orgs/{id}/service-accounts', { params: { path: { id: orgId } }, body, ...repeatable(key) }));
}

export function deleteServiceAccount(id: string): Promise<void> {
  return done(api.DELETE('/service-accounts/{id}', { params: { path: { id } } }));
}

export const serviceTokensKey = (id: string) => ['service-tokens', id] as const;

export const serviceTokensQuery = (id: string) =>
  queryOptions({
    queryKey: serviceTokensKey(id),
    queryFn: ({ signal }): Promise<ApiTokenInfo[]> => ok(api.GET('/service-accounts/{id}/tokens', { params: { path: { id } }, signal })),
  });

export function createServiceToken(id: string, body: ApiTokenCreateBody, key: string): Promise<TokenCreated> {
  return ok(api.POST('/service-accounts/{id}/tokens', { params: { path: { id } }, body, ...repeatable(key) }));
}

export function revokeServiceToken(id: string, tokenId: string): Promise<void> {
  return done(api.DELETE('/service-accounts/{id}/tokens/{token_id}', { params: { path: { id, token_id: tokenId } } }));
}

// ---------------------------------------------------------------- tokens that reach an org

export const orgTokensKey = (orgId: string) => ['org-tokens', orgId] as const;

/** The tokens that can reach anything of an org, with whether each is refused here or lives too long for it: safe facts, never a secret. */
export const orgTokensQuery = (orgId: string) =>
  infiniteQueryOptions({
    queryKey: orgTokensKey(orgId),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<OrgTokenPage> =>
      ok(
        api.GET('/orgs/{id}/tokens', {
          params: { path: { id: orgId }, query: { limit: PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** Refuses a token in an org and everything inside it; it works elsewhere as before. */
export function refuseOrgToken(orgId: string, tokenId: string): Promise<void> {
  return done(api.POST('/orgs/{id}/tokens/{token_id}/refuse', { params: { path: { id: orgId, token_id: tokenId } } }));
}

// ---------------------------------------------------------------- people from outside

export const outsidersKey = (orgId: string) => ['outsiders', orgId] as const;

/** The people from outside the tree who reach something of an org now, each with the first ways they do. */
export const outsidersQuery = (orgId: string, q: string) =>
  infiniteQueryOptions({
    queryKey: [...outsidersKey(orgId), q] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<OutsidePersonPage> =>
      ok(
        api.GET('/orgs/{id}/outsiders', {
          params: {
            path: { id: orgId },
            query: {
              limit: PAGE_SIZE,
              ...(q.trim() === '' ? {} : { q: q.trim() }),
              ...(pageParam === null ? {} : { cursor: pageParam }),
            },
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** The next page of the ways one person from outside reaches an org. */
export function outsiderSources(orgId: string, userId: string, cursor: string): Promise<OutsideSourcePage> {
  return ok(
    api.GET('/orgs/{id}/outsiders/{user_id}/sources', {
      params: { path: { id: orgId, user_id: userId }, query: { limit: PAGE_SIZE, cursor } },
    }),
  );
}

/** Plans taking a person from outside out of an org: the plan is kept, and its first page comes back. */
export function previewOutsiderRemoval(orgId: string, userId: string): Promise<OutsideRemovalPlan> {
  return ok(api.POST('/orgs/{id}/outsiders/{user_id}/remove/preview', { params: { path: { id: orgId, user_id: userId } } }));
}

/** The page of a kept removal plan that begins at `cursor`. */
export function outsiderRemovalPage(orgId: string, userId: string, planId: string, cursor: string): Promise<OutsideRemovalPlan> {
  return ok(
    api.GET('/orgs/{id}/outsiders/{user_id}/remove/plans/{plan_id}', {
      params: { path: { id: orgId, user_id: userId, plan_id: planId }, query: { cursor, limit: PAGE_SIZE } },
    }),
  );
}

/** Reads a removal plan whole, page by page: the confirmation needs the fingerprint of its last page. */
export async function wholeOutsiderRemoval(orgId: string, userId: string): Promise<OutsideRemovalPlan> {
  let plan = await previewOutsiderRemoval(orgId, userId);
  const removes = [...plan.removes];
  while (plan.next_cursor !== null) {
    plan = await outsiderRemovalPage(orgId, userId, plan.id, plan.next_cursor);
    removes.push(...plan.removes);
  }
  return { ...plan, removes };
}

/** Takes a person from outside out of an org as the plan read whole says. */
export function removeOutsider(orgId: string, userId: string, plan: OutsideRemovalPlan, reason: string): Promise<OutsideRemovalResult> {
  return ok(
    api.POST('/orgs/{id}/outsiders/{user_id}/remove', {
      params: { path: { id: orgId, user_id: userId } },
      body: { plan_id: plan.id, reviewed_fingerprint: plan.reviewed_fingerprint ?? '', reason: reason.trim() },
    }),
  );
}
