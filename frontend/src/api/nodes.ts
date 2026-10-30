/** API client for Hierarchy with orgs inside orgs, moves, archive, tags and a place's Activity.
 *
 * What the screens of places ask of the API: the root orgs a person belongs to, an org by its address
 * or by its id, the tree of a root org as the live connection loads and repairs it, one node with what
 * the caller holds on it, what lies directly in a node, the topics of a project by filter and page,
 * a place's Activity, and the acts that make, change, archive, move, delete and bring back a node.
 *
 * Projects lie in orgs and in other projects, where the screens call them folders; a topic lies in a project
 * or in a folder and is moved between them like a project is. A folder is made, moved and previewed by the
 * requests of a project.
 *
 * Every creating request carries the key of the act, so that a second attempt after a lost answer
 * makes nothing twice. A change to something with a revision carries the revision the person saw, and
 * a stale one comes back as `conflict.stale_revision`, which the screen shows and never retries alone.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, client, ok, repeatable } from '@/api/client';
import { type ResourceRows, resourceRows } from '@/api/listed';
import { queryClient } from '@/api/query';
import { settingsKey } from '@/api/settings';
import type {
  Access,
  ActivityEntryPage,
  ActivityKind,
  ArchivedFilter,
  AttentionFilter,
  CreateThread,
  DeletedNodePage,
  HiddenItem,
  Node,
  NodeArchivePlan,
  NodeArchiveResult,
  NodeDeleteBody,
  NodeDeleted,
  NodeDeletePlan,
  NodeMovePlan,
  NodeUpdateBody,
  NewNodeAccess,
  Org,
  OrgTreeGetReply,
  Project,
  SubscriptionLevel,
  Tag,
  Thread,
  Topic,
  TopicSort,
  TopicStatus,
  TopicStatusFilter,
  TreeNode,
} from '@/generated/api-types';

export const orgsKey = ['orgs'] as const;
export const orgBySlugKey = (slug: string) => ['org', slug] as const;
export const nodeKey = (id: string) => ['node', id] as const;
export const childrenKey = (id: string) => ['children', id] as const;
export const topicsKey = (projectId: string) => ['topics', projectId] as const;
export const activityKey = (id: string) => ['activity', id] as const;
export const tagsKey = (id: string) => ['tags', id] as const;
export const deletedKey = (id: string) => ['deleted', id] as const;

/** How many lines or rows a page of a list asks for: what a screen shows before "Show more". */
export const PAGE_SIZE = 50;

/** The root orgs the person owns or belongs to, their own org first. */
export const orgsQuery = queryOptions({
  queryKey: orgsKey,
  queryFn: ({ signal }): Promise<Org[]> => ok(api.GET('/orgs', { signal })),
});

/** An org by the short name of a root org, or by the address of an org inside one. */
export const orgBySlugQuery = (slug: string) =>
  queryOptions({
    queryKey: orgBySlugKey(slug),
    queryFn: ({ signal }): Promise<Org> =>
      ok(api.GET('/orgs/by-slug/{slug}', { params: { path: { slug } }, signal })),
  });

/** Any node by its id, with what the caller holds on it. */
export const nodeQuery = (id: string) =>
  queryOptions({
    queryKey: nodeKey(id),
    queryFn: ({ signal }): Promise<Node> => ok(api.GET('/nodes/{id}', { params: { path: { id } }, signal })),
  });

/** Explicitly reads current or retained facts to reconcile an uncertain write. */
export function readNode(id: string, inspectDeleted = false): Promise<Node> {
  return ok(api.GET('/nodes/{id}', { params: { path: { id }, ...(inspectDeleted ? { query: { inspect_deleted: true } } : {}) } }));
}

/** A project by its id: the same request as any node, narrowed to what a project screen reads. */
export const projectQuery = (id: string) =>
  queryOptions({
    queryKey: nodeKey(id),
    queryFn: ({ signal }): Promise<Project> =>
      ok(api.GET('/nodes/{id}', { params: { path: { id } }, signal })).then((node) => {
        if (node.kind !== 'project') throw new WrongKind('project', node.kind);
        return node;
      }),
  });

/** A topic by its id. */
export const topicQuery = (id: string) =>
  queryOptions({
    queryKey: nodeKey(id),
    queryFn: ({ signal }): Promise<Topic> =>
      ok(api.GET('/nodes/{id}', { params: { path: { id } }, signal })).then((node) => {
        if (node.kind !== 'topic') throw new WrongKind('topic', node.kind);
        return node;
      }),
  });

/** An address that names one kind of node was opened for another kind. */
export class WrongKind extends Error {
  override readonly name = 'WrongKind';
  readonly wanted: Node['kind'];
  readonly found: Node['kind'];

  constructor(wanted: Node['kind'], found: Node['kind']) {
    super(`wanted a ${wanted}, found a ${found}`);
    this.wanted = wanted;
    this.found = found;
  }
}

/**
 * A page of a listing as the screens read it: the rows its reader may read, with the minimal rows of
 * private things they administer kept apart on `items.privateRows` (the first page carries them).
 */
export interface ListedPage<T> {
  items: ResourceRows<T>;
  next_cursor: string | null;
}
export type NodePage = ListedPage<Node>;
export type TopicPage = ListedPage<Topic>;

/** A page as the API answers it, made into the page the screens read. */
function listedPage<T>(page: { items: readonly unknown[]; next_cursor?: string | null }): ListedPage<T> {
  return { items: resourceRows<T>(page.items as readonly (HiddenItem | (T & { content_hidden?: false }))[]), next_cursor: page.next_cursor ?? null };
}

/** What lies directly in an org: the orgs inside it and its projects, in the order they were made. */
export const childrenQuery = (id: string, archived: ArchivedFilter = 'exclude', participating = false) =>
  infiniteQueryOptions({
    queryKey: [...childrenKey(id), archived, participating] as const,
    initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<NodePage> =>
      listedPage<Node>(
        await ok(
          api.GET('/nodes/{id}/children', {
            params: { path: { id }, query: { archived, limit: PAGE_SIZE, ...(participating ? { participating: true } : {}), ...(pageParam === null ? {} : { cursor: pageParam }) } },
            signal,
          }),
        ),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** What the topic list of a project is asked with; everything but the project is optional. */
export interface TopicFilters {
  participating?: boolean;
  status: TopicStatusFilter;
  /** The identifiers of tags, all of which a topic must carry. */
  tags: readonly string[];
  attention: AttentionFilter | 'needs_attention' | null;
  sort: TopicSort;
  q: string;
  archived: ArchivedFilter;
  /** Descendant topics are permission-filtered and paged by the server, using the same scope as the board. */
  withFolders?: boolean;
}

export const DEFAULT_TOPIC_FILTERS: TopicFilters = {
  status: 'open',
  tags: [],
  attention: null,
  sort: 'activity',
  q: '',
  archived: 'exclude',
};

/** The topics of a project, a page at a time, as the filters say. */
export const topicsQuery = (projectId: string, filters: TopicFilters, limit: number = PAGE_SIZE) =>
  infiniteQueryOptions({
    queryKey: [...topicsKey(projectId), filters, limit] as const,
    initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<TopicPage> =>
      listedPage<Topic>(
        await ok(
          api.GET('/projects/{id}/topics', {
            params: {
              path: { id: projectId },
              query: {
                status: filters.status,
                sort: filters.sort,
                archived: filters.archived,
                limit,
                ...(filters.tags.length > 0 ? { tag_id: [...filters.tags] } : {}),
                ...(filters.attention === null ? {} : { attention: filters.attention }),
                ...(filters.withFolders === true ? { with_folders: true } : {}),
                ...(filters.participating === true ? { participating: true } : {}),
                ...(filters.q.trim() === '' ? {} : { q: filters.q.trim() }),
                ...(pageParam === null ? {} : { cursor: pageParam }),
              },
            },
            signal,
          }),
        ),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** A place's Activity, newest first, a page at a time; `kinds` keeps to some kinds of line. */
export const activityQuery = (id: string, kinds: readonly ActivityKind[] = [], depth: 'summary' | 'every' = 'summary') =>
  infiniteQueryOptions({
    queryKey: [...activityKey(id), kinds, depth] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<ActivityEntryPage> =>
      ok(
        api.GET('/nodes/{id}/activity', {
          params: {
            path: { id },
            query: {
              limit: PAGE_SIZE,
              depth,
              ...(kinds.length > 0 ? { kind: [...kinds] } : {}),
              ...(pageParam === null ? {} : { cursor: pageParam }),
            },
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** Replaces the assigned tags of any place using the node's revision fence. */
export function setNodeTags(id: string, tagIds: string[], revision: number): Promise<Node> {
  return ok(api.PUT('/nodes/{id}/tags', { params: { path: { id } }, body: { tag_ids: tagIds, expected_revision: revision } }));
}

/** Tags defined here and along the path above this place. */
export const tagsQuery = (id: string) =>
  queryOptions({
    queryKey: tagsKey(id),
    queryFn: ({ signal }): Promise<Tag[]> => ok(api.GET('/nodes/{id}/tags', { params: { path: { id } }, signal })),
  });

/** What was deleted directly under a node and is not purged yet. */
export const deletedQuery = (id: string) =>
  infiniteQueryOptions({
    queryKey: deletedKey(id),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<DeletedNodePage> =>
      ok(
        api.GET('/nodes/{id}/deleted', {
          params: { path: { id }, query: { limit: PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/**
 * The nodes of a root org's tree that the caller may view: all of them, or with `since` those that
 * changed after that sequence. This is the request the tree's live feed is loaded and repaired by.
 */
export async function fetchTree(orgId: string, since: number | null, signal?: AbortSignal): Promise<TreeReply> {
  const reader = client.session.getState().readerVersion;
  let reply: OrgTreeGetReply = await ok(
    api.GET('/orgs/{id}/tree', {
      params: { path: { id: orgId }, query: since === null ? {} : { since } },
      ...(signal ? { signal } : {}),
    }),
  );
  const seq = reply.seq;
  signal?.throwIfAborted();
  if (client.session.getState().readerVersion !== reader) throw new DOMException('The tree reader changed', 'AbortError');
  // Prime an absent observer only. A setting already supplied by a newer live/native read wins.
  if (reply.settings !== undefined && reply.settings !== null && queryClient.getQueryData(settingsKey(orgId)) === undefined) {
    queryClient.setQueryData(settingsKey(orgId), reply.settings);
  }
  const rows = [...reply.nodes];
  while (since === null && reply.next_cursor !== null && reply.next_cursor !== undefined) {
    reply = await ok(api.GET('/orgs/{id}/tree', {
      params: { path: { id: orgId }, query: { cursor: reply.next_cursor } },
      ...(signal ? { signal } : {}),
    }));
    rows.push(...reply.nodes);
  }
  return { ...reply, seq, ...splitListed(rows) };
}

/** A tree as the page holds it: the rows its reader may read, and apart from them the minimal rows of private places they administer. */
export type TreeReply = Omit<OrgTreeGetReply, 'nodes'> & TreeRows;

/** Rows of a tree, readable and minimal apart. */
export interface TreeRows {
  nodes: TreeNode[];
  hidden: HiddenItem[];
}

/** Splits the rows of a tree, as a request or the live feed brings them, into readable rows and minimal ones, as every listing does. */
export function splitListed(rows: readonly unknown[]): TreeRows {
  const split = resourceRows<TreeNode>(rows as readonly (HiddenItem | (TreeNode & { content_hidden?: false }))[]);
  return { nodes: [...split], hidden: split.privateRows };
}

// ---------------------------------------------------------------- making

/** Makes a root org of the kind a team has; its maker is its first owner. */
export function createOrg(body: { name: string; slug: string }, key: string): Promise<Org> {
  return ok(api.POST('/orgs', { body, ...repeatable(key) }));
}

/** Makes an org inside another, which people call a department. */
export function createDepartment(
  parentId: string,
  body: { name: string; slug: string; description?: string; general?: boolean; access?: NewNodeAccess },
  key: string,
): Promise<Org> {
  return ok(api.POST('/orgs/{id}/orgs', { params: { path: { id: parentId } }, body, ...repeatable(key) }));
}

/** Makes a project directly in an org, at whatever level it lies. */
export function createProject(
  orgId: string,
  body: { name: string; description?: string; access?: NewNodeAccess },
  key: string,
): Promise<Project> {
  return ok(api.POST('/orgs/{id}/projects', { params: { path: { id: orgId } }, body, ...repeatable(key) }));
}

/** Makes a folder: a project inside a project, beside its topics. It is a project like any other, made by whoever opens topics there. */
export function createFolder(
  projectId: string,
  body: { name: string; description?: string; access?: NewNodeAccess },
  key: string,
): Promise<Project> {
  return ok(api.POST('/projects/{id}/projects', { params: { path: { id: projectId } }, body, ...repeatable(key) }));
}

/** Makes a topic in a project or in a folder; without a name it is `New topic` and its title is kept by automation. */
export async function createTopic(
  projectId: string,
  body: { name?: string; description?: string; access?: NewNodeAccess },
  key: string,
): Promise<Topic> {
  const reply = await ok(api.POST('/projects/{id}/topics', { params: { path: { id: projectId } }, body, ...repeatable(key) }));
  // Asked without a first thread, the answer is the topic alone.
  return 'topic' in reply ? reply.topic : reply;
}

/** Admits a project's new topic and its first thread (including a new environment) as one repeatable act. */
export async function createTopicAndThread(projectId: string, firstThread: CreateThread, key: string): Promise<Thread> {
  const reply = await ok(api.POST('/projects/{id}/topics', {
    params: { path: { id: projectId } }, body: { first_thread: firstThread }, ...repeatable(key),
  }));
  if (!('topic' in reply)) throw new Error('the accepted first thread is missing from the topic reply');
  return reply.thread;
}

// ---------------------------------------------------------------- changing

/** Renames a node or changes its description, on the revision the person saw. */
export function updateNode(id: string, body: NodeUpdateBody): Promise<Node> {
  return ok(api.PATCH('/nodes/{id}', { params: { path: { id } }, body }));
}

/** Sets a topic's status or its tags. */
export function updateTopic(id: string, body: { status?: TopicStatus; tag_ids?: string[]; expected_revision?: number }): Promise<Topic> {
  return ok(api.PATCH('/topics/{id}', { params: { path: { id } }, body }));
}

/** Reads the complete consequences and exact stamp; it settles no work. */
export function previewArchive(id: string, archiving = true): Promise<NodeArchivePlan> {
  const options = { params: { path: { id } } };
  return archiving ? ok(api.POST('/nodes/{id}/archive/preview', options)) : ok(api.POST('/nodes/{id}/unarchive/preview', options));
}

/** Applies only the review shown; its inverse is fenced to this success. */
export function applyArchivePlan(plan: NodeArchivePlan, key?: string): Promise<NodeArchiveResult> {
  const options = { params: { path: { id: plan.node.id } }, body: { preflight_id: plan.preflight_id, stamp: plan.stamp, expected_revision: plan.revision }, ...(key === undefined ? {} : repeatable(key)) };
  return plan.archiving ? ok(api.POST('/nodes/{id}/archive', options)) : ok(api.POST('/nodes/{id}/unarchive', options));
}

export async function archiveNode(id: string): Promise<Node> {
  return (await applyArchivePlan(await previewArchive(id))).node;
}

export async function unarchiveNode(id: string): Promise<Node> {
  return (await applyArchivePlan(await previewArchive(id, false))).node;
}

/** Deletes a node with everything in it; it stays recoverable until it is purged. Says the group of what it deleted, which its Undo names. */
export function deleteNode(id: string, body: NodeDeleteBody = {}, key?: string): Promise<NodeDeleted> {
  return ok(api.DELETE('/nodes/{id}', { params: { path: { id } }, body, ...(key === undefined ? {} : repeatable(key)) }));
}

/** What deleting a node would remove, until when and by whom it can be brought back: a plan the deletion names. Nothing is deleted. */
export function previewDelete(id: string): Promise<NodeDeletePlan> {
  return ok(api.POST('/nodes/{id}/delete/preview', { params: { path: { id } } }));
}

/**
 * Brings back what was deleted, archived, with everything under it. With the group of a deletion (`undoDelete`) it is
 * that deletion's Undo: each thing comes back as it stood before, archived or not, or nothing comes back when a newer
 * deletion or a change came between.
 */
export function restoreNode(id: string, undoDelete?: string | null): Promise<Node> {
  return ok(api.POST('/nodes/{id}/restore', { params: { path: { id } }, body: undoDelete === undefined || undoDelete === null ? {} : { undo_delete: undoDelete } }));
}

/** What can be moved: a project or a folder, a topic, and an org that lies in another. */
export type MovableKind = 'project' | 'org' | 'topic';

/** What moving a project, a folder, a topic or an org that lies in another into another place of its tree would change, before anything does. */
export function previewMove(kind: MovableKind, id: string, parentId: string): Promise<NodeMovePlan> {
  const body = { parent_id: parentId };
  const path = { params: { path: { id } }, body };
  switch (kind) {
    case 'project':
      return ok(api.POST('/projects/{id}/move/preview', path));
    case 'topic':
      return ok(api.POST('/topics/{id}/move/preview', path));
    case 'org':
      return ok(api.POST('/orgs/{id}/move/preview', path));
  }
}

/**
 * Moves a project or a folder to another org or into another project of its tree, a topic to another project or
 * folder, an org that lies in another under another org: on the revision the person saw.
 */
export function moveNode(kind: MovableKind, id: string, parentId: string, revision: number, plan?: NodeMovePlan, key?: string): Promise<Node> {
  const body = { parent_id: parentId, expected_revision: revision, ...(plan === undefined ? {} : { preflight_id: plan.preflight_id }) };
  const path = { params: { path: { id } }, body, ...(key === undefined ? {} : repeatable(key)) };
  switch (kind) {
    case 'project':
      return ok(api.POST('/projects/{id}/move', path));
    case 'topic':
      return ok(api.POST('/topics/{id}/move', path));
    case 'org':
      return ok(api.POST('/orgs/{id}/move', path));
  }
}

/**
 * How much of what happens in a node the caller is told of: their own row, or with `null` none of
 * their own, so that the nearest one above decides. The answer is what the caller holds there now.
 */
export function setSubscription(id: string, level: SubscriptionLevel | null): Promise<Access> {
  return level === null
    ? ok(api.DELETE('/nodes/{id}/subscription', { params: { path: { id } } }))
    : ok(api.PUT('/nodes/{id}/subscription', { params: { path: { id } }, body: { level } }));
}
