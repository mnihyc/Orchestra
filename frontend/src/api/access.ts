/** API client for Roles held in places, permission pages with their owners and defaults pages, the change that names owners and holders at a choice, a thing's history, and access requests for everything missing at once.
 *
 * The catalogue of permissions as data, by which every page is drawn and a refusal names what is
 * missing in the words of the system; the page of any node, document, resource or chat in one shape
 * and the page of defaults a place keeps for what is made inside it; the one change that edits either,
 * applied as a whole or not at all against the revision the person saw; what a change would do to
 * other people before it is made; the people behind one holder of a page, a page at a time; what one
 * person holds on a thing and through what; the roles held in a place; and asking for access.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { ApiError, api, done, ok, repeatable } from '@/api/client';
import type {
  AccessRequestCreateBody,
  AccessRequestCreateReply,
  AccessRequestGetReply,
  Approval,
  EffectiveAccess,
  EnvironmentReach,
  Group,
  HistoryLinePage,
  Holder,
  HolderInput,
  HolderSearchReply,
  NodeKind,
  NodeRoleListReply,
  PageChange,
  PagePreview,
  PermissionCatalogueGetReply,
  PermissionPage,
  PermissionPeopleListReply,
  Role,
  RoleCreateBody,
  RoleHeld,
  SharePreview,
  SharePreviewBody,
  SubjectInput,
  UserRef,
} from '@/generated/api-types';

// ---------------------------------------------------------------- what a page is of

/** The kinds of thing that have a permission page: the nodes, a document, a chat and the seven kinds of resource. */
export type AccessKind =
  | NodeKind
  | 'document'
  | 'document_container'
  | 'chat'
  | 'provider'
  | 'agent'
  | 'environment'
  | 'host'
  | 'machine'
  | 'template'
  | 'mcp_server'
  | 'mount'
  | 'skill';

/** Where a kind of thing is addressed in the paths of the API. */
const PATHS: Record<AccessKind, string> = {
  org: 'nodes',
  project: 'nodes',
  topic: 'nodes',
  thread: 'nodes',
  document: 'documents',
  document_container: 'document-containers',
  chat: 'chats',
  provider: 'providers',
  agent: 'agents',
  environment: 'environments',
  host: 'hosts',
  machine: 'machines',
  template: 'templates',
  mcp_server: 'mcp-servers',
  mount: 'mounts',
  skill: 'skills',
};

/** The thing a page is of, as every screen that opens it knows it. */
export interface AccessTarget {
  kind: AccessKind;
  id: string;
  /** What it is called, for the heading. */
  name: string;
  /** The org it lies in (a node) or has as its home (a resource), where the caller knows it. */
  orgId?: string | undefined;
}

/** The system's defaults are a real scope, not a node with a made-up identifier. */
export interface SystemAccessTarget { kind: 'system'; id: 'system'; name: string }
export type AccessRowTarget = AccessTarget | SystemAccessTarget;

/** Whether the kind is a node of the hierarchy, independently of its permission-page readers. */
export function isNodeKind(kind: string): kind is NodeKind {
  return kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread';
}

/** Whether the kind is a place: where roles are held and defaults are kept. */
export function isPlaceKind(kind: AccessKind): kind is 'org' | 'project' | 'topic' {
  return kind === 'org' || kind === 'project' || kind === 'topic';
}

export const pathOf = (kind: AccessKind): string => PATHS[kind];

// ---------------------------------------------------------------- the catalogue

export const permissionsKey = ['permissions'] as const;
export const rolesKey = (nodeId: string) => ['roles', nodeId] as const;

/** Every permission with its name in words and what it allows. It changes with a release, so it is read once. */
export const permissionsQuery = queryOptions({
  queryKey: permissionsKey,
  queryFn: ({ signal }): Promise<PermissionCatalogueGetReply> => ok(api.GET('/permissions', { signal })),
  staleTime: Number.POSITIVE_INFINITY,
});

// ---------------------------------------------------------------- pages

export const pageKey = (target: Pick<AccessTarget, 'kind' | 'id'>) => ['page', pathOf(target.kind), target.id] as const;
export const defaultsKey = (nodeId: string) => ['defaults', nodeId] as const;
export const systemDefaultsKey = ['defaults', 'system'] as const;

export const readSystemDefaults = (signal?: AbortSignal): Promise<PermissionPage> => ok(api.GET('/admin/defaults', { signal }));
export const changeSystemDefaults = (change: PageChange): Promise<PermissionPage> => ok(api.PATCH('/admin/defaults', { body: change }));
export const previewSystemDefaults = (change: PageChange): Promise<PagePreview> => ok(api.POST('/admin/defaults/preview', { body: change }));

/** Reads the page of a thing: every row, what each inherits and from where, whom it names, and what the caller may change. */
export function readPage(target: Pick<AccessTarget, 'kind' | 'id'>, signal?: AbortSignal): Promise<PermissionPage> {
  return ok(api.GET('/{object}/{id}/permissions', { params: { path: { object: pathOf(target.kind), id: target.id } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Reads what a place gives to what is made inside it, with both rows of each permission that has two. */
export function readDefaults(nodeId: string, signal?: AbortSignal): Promise<PermissionPage> {
  return ok(api.GET('/nodes/{id}/defaults', { params: { path: { id: nodeId } }, ...(signal === undefined ? {} : { signal }) }));
}

/** The page of a thing, kept. */
export const pageQuery = (target: Pick<AccessTarget, 'kind' | 'id'>) =>
  queryOptions({ queryKey: pageKey(target), queryFn: ({ signal }): Promise<PermissionPage> => readPage(target, signal) });

/** The defaults of a place, kept. */
export const defaultsQuery = (nodeId: string) =>
  queryOptions({ queryKey: defaultsKey(nodeId), queryFn: ({ signal }): Promise<PermissionPage> => readDefaults(nodeId, signal) });

/** Changes a page, as a whole or not at all; a revision the page has moved past comes back as a stale one. */
export function changePage(target: Pick<AccessTarget, 'kind' | 'id'>, change: PageChange, key?: string): Promise<PermissionPage> {
  return ok(api.PATCH('/{object}/{id}/permissions', { params: { path: { object: pathOf(target.kind), id: target.id } }, body: change, ...(key === undefined ? {} : repeatable(key)) }));
}

/** Changes the page of defaults of a place in the same way. */
export function changeDefaults(nodeId: string, change: PageChange): Promise<PermissionPage> {
  return ok(api.PATCH('/nodes/{id}/defaults', { params: { path: { id: nodeId } }, body: change }));
}

/** Says what a change would do to other people, with nothing changed. */
export function previewPage(target: Pick<AccessTarget, 'kind' | 'id'>, change: PageChange): Promise<PagePreview> {
  return ok(api.POST('/{object}/{id}/permissions/preview', { params: { path: { object: pathOf(target.kind), id: target.id } }, body: change }));
}

/** Reviews an ordinary page, role or agent share; it applies nothing and the write still rechecks its own authority. */
export function previewShare(target: Pick<AccessTarget, 'kind' | 'id'>, body: SharePreviewBody): Promise<SharePreview> {
  return ok(api.POST('/{object}/{id}/share/preview',{params:{path:{object:pathOf(target.kind),id:target.id}},body}));
}

/** The same for a change to the defaults of a place: how many pages follow the rows, and who gains or loses what. */
export function previewDefaults(nodeId: string, change: PageChange): Promise<PagePreview> {
  return ok(api.POST('/nodes/{id}/defaults/preview', { params: { path: { id: nodeId } }, body: change }));
}

/** The page the server holds now, which a refusal for a stale revision carries; `null` when it carries none. */
export function currentPageOf(error: unknown): PermissionPage | null {
  if (!(error instanceof ApiError) || !error.is('conflict.stale_revision')) return null;
  const current = error.problem.current;
  if (typeof current !== 'object' || current === null) return null;
  const page = current as Partial<PermissionPage>;
  return Array.isArray(page.rows) && typeof page.revision === 'number' ? (current as PermissionPage) : null;
}

// ---------------------------------------------------------------- the people behind a chip, what one person holds, history

/** How many people a chip asks for at a time: what a hover card shows without scrolling. */
export const PEOPLE_PAGE = 8;

/** The people behind one holder of a page, a page at a time. `holder` is the form the API names (`role:<place>:<role>`, `group:<id>`, ...). */
export const peopleQuery = (target: Pick<AccessTarget, 'kind' | 'id'>, holder: string) =>
  infiniteQueryOptions({
    queryKey: ['page-people', pathOf(target.kind), target.id, holder] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<PermissionPeopleListReply> =>
      ok(
        api.GET('/{object}/{id}/permissions/people', {
          params: {
            path: { object: pathOf(target.kind), id: target.id },
            query: { holder, limit: PEOPLE_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) },
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
    // Automatic retries cannot change whether the page shows this holder.
    retry: false,
  });

/** The form the API names a holder in when it is asked for the people behind it; `null` for the holders that are no group of people. */
export function holderParam(holder: Holder): string | null {
  switch (holder.kind) {
    case 'role':
      return `role:${holder.at?.id ?? 'local'}:${holder.role}`;
    case 'group':
      return `group:${holder.group.id}`;
    case 'node':
      return `node:${holder.node.id}`;
    case 'members':
      return `members:${holder.node.id}`;
    default:
      return null;
  }
}

/** What one person holds on a thing, and what gives it to them. */
export const effectiveQuery = (target: Pick<AccessTarget, 'kind' | 'id'>, userId: string) =>
  queryOptions({
    queryKey: ['page-effective', pathOf(target.kind), target.id, userId] as const,
    queryFn: ({ signal }): Promise<EffectiveAccess> =>
      ok(
        api.GET('/{object}/{id}/permissions/effective', {
          params: { path: { object: pathOf(target.kind), id: target.id }, query: { user_id: userId } },
          signal,
        }),
      ),
    retry: false,
  });

/** What was done to a thing and by whom, newest first, a page at a time. */
export const historyQuery = (target: Pick<AccessTarget, 'kind' | 'id'>, depth: 'summary' | 'every' = 'every', actor?: string) =>
  infiniteQueryOptions({
    queryKey: ['page-history', pathOf(target.kind), target.id, depth, actor ?? null] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<HistoryLinePage> =>
      ok(
        api.GET('/{object}/{id}/history', {
          params: {
            path: { object: pathOf(target.kind), id: target.id },
            query: { limit: PEOPLE_PAGE, depth, ...(actor === undefined ? {} : { actor }), ...(pageParam === null ? {} : { cursor: pageParam }) },
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
    retry: false,
  });

// ---------------------------------------------------------------- finding whom to name

/** The fewest characters the server searches for. */
export const SEARCH_MIN_CHARS = 2;

/** One holder search: names, whole or partial locators, and predefined holders of the chosen place. */
export const holderSearchQuery = (q: string, placeId: string | null, orgId: string | null, includeWork: boolean, listPlaces = false) =>
  queryOptions({ queryKey: ['holder-search', q, placeId, orgId, includeWork, listPlaces] as const,
    queryFn: ({ signal }): Promise<HolderSearchReply> => ok(api.GET('/holders/search', { params: { query: {
      q, ...(placeId === null ? {} : { place_id: placeId }), ...(orgId === null ? {} : { org_id: orgId }), include_work: includeWork, list_places: listPlaces,
    } }, signal })), staleTime: 15_000 });

/** A chosen permission holder as the subject of a role, retaining its source place. */
export function roleSubject(input: HolderInput, placeId: string): SubjectInput | null {
  switch (input.kind) {
    case 'user': case 'group': case 'members': return { kind: input.kind, id: input.id };
    case 'role': return { kind: 'role', id: input.at ?? placeId, role: input.role };
    default: return null;
  }
}

/** People by name, among the members of an org when one is given, and an exact username or address anywhere. */
export const searchPeopleQuery = (q: string, orgId: string | null) =>
  queryOptions({
    queryKey: ['people-search', q, orgId] as const,
    queryFn: ({ signal }): Promise<UserRef[]> =>
      ok(api.GET('/users/search', { params: { query: { q, ...(orgId === null ? {} : { org_id: orgId }) } }, signal })),
    enabled: q.length >= SEARCH_MIN_CHARS,
    staleTime: 15_000,
  });

/** The groups kept by the org containing the chosen place. */
export const groupsQuery = (orgId: string) =>
  queryOptions({
    queryKey: ['groups', orgId] as const,
    queryFn: ({ signal }): Promise<Group[]> =>
      ok(api.GET('/nodes/{id}/groups', { params: { path: { id: orgId } }, signal })),
  });

// ---------------------------------------------------------------- roles held in a place

/** Who holds which role in a place, and who holds one in a place above it. */
export const rolesQuery = (nodeId: string) =>
  queryOptions({
    queryKey: rolesKey(nodeId),
    queryFn: ({ signal }): Promise<NodeRoleListReply> =>
      ok(api.GET('/nodes/{id}/roles', { params: { path: { id: nodeId } }, signal })),
  });

/** Gives a role held already another one, on the revision of the row the person saw. */
export function changeRole(roleId: string, role: Role, revision: number): Promise<RoleHeld> {
  return ok(api.PATCH('/roles/{id}', { params: { path: { id: roleId } }, body: { role, expected_revision: revision } }));
}

/** Changes the end of a role, or the note beside it, on the revision of the row the person saw. */
export function changeRoleDetails(
  roleId: string,
  revision: number,
  details: { expires_at?: string | null; note?: string },
): Promise<RoleHeld> {
  return ok(api.PATCH('/roles/{id}', { params: { path: { id: roleId } }, body: { expected_revision: revision, ...details } }));
}

/** Gives a role in a place to a person, a group or the members of an org. */
export function giveRole(nodeId: string, body: RoleCreateBody): Promise<RoleHeld> {
  return ok(api.POST('/nodes/{id}/roles', { params: { path: { id: nodeId } }, body }));
}

/** Takes a role away. */
export function takeRole(roleId: string): Promise<void> {
  return done(api.DELETE('/roles/{id}', { params: { path: { id: roleId } } }));
}

// ---------------------------------------------------------------- asking for access

/** Asks for what is missing, one approval for each thing, each sent to the people who can give it. */
export function requestAccess(body: AccessRequestCreateBody, key: string): Promise<AccessRequestCreateReply> {
  return ok(api.POST('/access-requests', { body, ...repeatable(key) }));
}

/** Asks to join a known group; the grant is ordinary membership. */
export function requestJoin(chatId:string,body:{reason:string;label?:string;expires_at?:string},key:string):Promise<AccessRequestCreateReply> {
  return ok(api.POST('/chats/{id}/join-requests',{params:{path:{id:chatId}},body,...repeatable(key)}));
}

/** Where a batch of requests stands. */
export const accessRequestQuery = (batchId: string) =>
  queryOptions({
    queryKey: ['access-request', batchId] as const,
    queryFn: ({ signal }): Promise<AccessRequestGetReply> =>
      ok(api.GET('/access-requests/{batch_id}', { params: { path: { batch_id: batchId } }, signal })),
    retry: false,
  });

/** Takes a request back before it is decided. */
export function cancelApproval(approvalId: string): Promise<Approval> {
  return ok(api.POST('/approvals/{id}/cancel', { params: { path: { id: approvalId } } }));
}

// ---------------------------------------------------------------- the requests that wait on a page

export const waitingRequestsKey = (nodeId: string) => ['access-requests', 'waiting', nodeId] as const;

/** The requests for access that wait on a place or on anything below it, which the caller may see; each says whether the caller may decide it. */
export const waitingRequestsQuery = (nodeId: string) =>
  queryOptions({
    queryKey: waitingRequestsKey(nodeId),
    queryFn: async ({ signal }): Promise<Approval[]> => {
      const page = await ok(api.GET('/approvals', { params: { query: { scope: `node:${nodeId}`, kind: 'access', state: 'pending' } }, signal }));
      return page.items;
    },
    // A system that does not list them says nothing of them here.
    retry: false,
  });

// ---------------------------------------------------------------- who reaches into an environment

export const environmentReachKey = (environmentId: string) => ['environment-reach', environmentId] as const;

/** Everybody who can reach into an environment now, and how: through which of its doors, to look or to reach in, or through an agent there that they may talk to. */
export const environmentReachQuery = (environmentId: string) =>
  queryOptions({
    queryKey: environmentReachKey(environmentId),
    queryFn: ({ signal }): Promise<EnvironmentReach> => ok(api.GET('/environments/{id}/reach', { params: { path: { id: environmentId } }, signal })),
    retry: false,
  });
