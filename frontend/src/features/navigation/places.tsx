/** What a place is to the left bar and to every menu that acts on one: its kind, its name and where it lies, its address and its icon.
 *
 * A place is an org, a project, a topic or a thread. The tree of the left bar, the node a screen has
 * read and a favorite each describe one in their own shape; every menu and dialog of places takes
 * this one, made from any of them. An org is addressed by its id, which its screen replaces by its
 * address, because the tree carries no address for an org inside another.
 *
 * A project that lies in a project is called a folder, in every text and by its own glyph. It is a
 * project like any other underneath, so `kind` stays `project`; `folder` says where it lies, which
 * no row or reference says alone: the kind of what it lies in has to be told.
 */
import { Link, useNavigate } from '@tanstack/react-router';
import type { LucideIcon } from 'lucide-react';
import { type ComponentProps, type ReactNode, useMemo } from 'react';

import type { Favorite, Node, NodeKind, NodeRef, TopicStatus, TreeNode, UserRef } from '@/generated/api-types';
import { type OrgTree, pathTo, useTreeStore } from '@/realtime/stores/tree';

import { ACTIONS } from '@/ui/actions';

export interface PlaceRef {
  id: string;
  kind: NodeKind;
  name: string;
  parentId: string | null;
  archived: boolean;
  /** For a topic: whether work goes on. */
  status: TopicStatus | null;
  /** For a project: whether it lies in a project, which makes it a folder. */
  folder: boolean;
}

/** A row of the tree; `folder` is told by the caller, who knows the kind of the row it lies under. */
export function placeOfRow(row: TreeNode, folder = false): PlaceRef {
  return {
    id: row.id,
    kind: row.kind,
    name: row.name,
    parentId: row.parent_id,
    archived: row.archived,
    status: row.topic?.status ?? null,
    folder: row.kind === 'project' && folder,
  };
}

/** Whether a path, from the root org to a node, ends in a project that lies in a project. */
export function isFolderPath(path: readonly Pick<NodeRef, 'kind'>[]): boolean {
  return path.at(-1)?.kind === 'project' && path.at(-2)?.kind === 'project';
}

export function placeOfNode(node: Node): PlaceRef {
  return {
    id: node.id,
    kind: node.kind,
    name: node.name,
    parentId: node.parent_id,
    archived: node.archived,
    status: node.kind === 'topic' ? node.status : null,
    folder: isFolderPath(node.path),
  };
}

/** A node named by another thing (a favorite's, an activity line's) is a place whose parent is not known from it alone. */
export function placeOfRef(ref: NodeRef, parent: Pick<NodeRef, 'id' | 'kind'> | null = null): PlaceRef | null {
  if (ref.name === null) return null;
  return { id: ref.id, kind: ref.kind, name: ref.name, parentId: parent?.id ?? null, archived: false, status: null, folder: ref.kind === 'project' && parent?.kind === 'project' };
}

export function placeOfFavorite(favorite: Favorite): PlaceRef | null {
  return placeOfRef(favorite.node, favorite.path.at(-1) ?? null);
}

/** The word a text uses for what a place is: a project that lies in a project is a folder. */
export type KindWord = NodeKind | 'folder';

export function kindWord(place: Pick<PlaceRef, 'kind' | 'folder'>): KindWord {
  return place.kind === 'project' && place.folder ? 'folder' : place.kind;
}

/** Where a place's screen is, as a path inside the application. */
export function placePath(place: Pick<PlaceRef, 'id' | 'kind'>): string {
  switch (place.kind) {
    case 'org':
      return `/org/${place.id}`;
    case 'project':
      return `/project/${place.id}`;
    case 'topic':
      return `/topic/${place.id}`;
    case 'thread':
      return `/thread/${place.id}`;
  }
}

/** The full address of a place's screen, for a link that is passed on. */
export function placeUrl(place: Pick<PlaceRef, 'id' | 'kind'>): string {
  return new URL(placePath(place), location.origin).href;
}

/**
 * The glyph of each kind of place, bare: a folder has the folder, and a project a glyph of its own that is no folder.
 * Where a place's icon is drawn it is `NodeIcon`, which gives the glyph its colour; this is for what takes a glyph alone.
 */
export const PLACE_ICONS: Record<KindWord, LucideIcon> = {
  org: ACTIONS.openOrg.Icon,
  project: ACTIONS.openProject.Icon,
  folder: ACTIONS.openFolder.Icon,
  topic: ACTIONS.openTopic.Icon,
  thread: ACTIONS.openThread.Icon,
};

/** Where a document's page is, as a path inside the application. */
export const documentPath = (id: string): string => `/doc/${id}`;

/** A document as the left bar's menus and dialogs need it, whatever named it: a row of the tree, or a name the person went to lately. */
export interface DocPlace {
  id: string;
  title: string;
  /** The place it is written beside. */
  node: Pick<NodeRef, 'id' | 'kind'>;
  /** Whether that place is archived: the document is then read and not written. */
  archived: boolean;
  /** Its first owner, whom to ask for what the caller may not do with it; `null` where what named it did not say. */
  owner: Pick<UserRef, 'display_name'> | null;
}

/**
 * The colour somebody gave a place's icon, from the trees that are held: `null` for the colour of its kind, and
 * for a place no tree held knows. Whatever names a place by its id alone (a name the finder found, a favorite)
 * draws its icon with this.
 */
export function useNodeColor(id: string | null | undefined): number | null {
  return useTreeStore((state) => {
    if (id === null || id === undefined) return null;
    for (const tree of state.byOrg.values()) {
      const row = tree.nodes.get(id);
      if (row !== undefined) return row.color;
    }
    return null;
  });
}

type AnchorProps = Omit<ComponentProps<'a'>, 'href' | 'children'>;

/** A place's own icon colour from a held tree, including a saved finder favourite. */
export function useNodeOwnColor(id: string): string | null {
  return useTreeStore((state) => {
    for (const tree of state.byOrg.values()) {
      const row = tree.nodes.get(id);
      if (row !== undefined) return row.custom_color ?? null;
    }
    return null;
  });
}

/** A real link to a place's screen: it opens in a new tab on a middle click, as every link does. */
export function PlaceLink({ place, children, ...anchor }: AnchorProps & { place: Pick<PlaceRef, 'id' | 'kind'>; children: ReactNode }) {
  switch (place.kind) {
    case 'org':
      return (
        <Link to="/org/$slug" params={{ slug: place.id }} {...anchor}>
          {children}
        </Link>
      );
    case 'project':
      return (
        <Link to="/project/$projectId" params={{ projectId: place.id }} {...anchor}>
          {children}
        </Link>
      );
    case 'topic':
      return (
        <Link to="/topic/$topicId" params={{ topicId: place.id }} {...anchor}>
          {children}
        </Link>
      );
    case 'thread':
      return (
        <Link to="/thread/$threadId" params={{ threadId: place.id }} {...anchor}>
          {children}
        </Link>
      );
  }
}

/** Goes to a place's screen. */
export function useGoToPlace(): (place: Pick<PlaceRef, 'id' | 'kind'>) => void {
  const navigate = useNavigate();
  return (place) => {
    switch (place.kind) {
      case 'org':
        void navigate({ to: '/org/$slug', params: { slug: place.id } });
        return;
      case 'project':
        void navigate({ to: '/project/$projectId', params: { projectId: place.id } });
        return;
      case 'topic':
        void navigate({ to: '/topic/$topicId', params: { topicId: place.id } });
        return;
      case 'thread':
        void navigate({ to: '/thread/$threadId', params: { threadId: place.id } });
        return;
    }
  };
}

/** Every row that lies under a node in a tree, itself not included and deleted rows left out, in no particular order. */
export function* descendantsOf(rows: ReadonlyMap<string, TreeNode>, id: string): Generator<TreeNode> {
  const children = new Map<string, TreeNode[]>();
  for (const row of rows.values()) {
    if (row.deleted || row.parent_id === null) continue;
    const list = children.get(row.parent_id);
    if (list === undefined) children.set(row.parent_id, [row]);
    else list.push(row);
  }
  const pending = [...(children.get(id) ?? [])];
  for (let row = pending.pop(); row !== undefined; row = pending.pop()) {
    yield row;
    pending.push(...(children.get(row.id) ?? []));
  }
}

/** What lies under a place in a tree: how many of each kind, and how many threads are running. A project that lies in a project is counted as a folder. */
export interface SubtreeCounts {
  orgs: number;
  projects: number;
  folders: number;
  topics: number;
  threads: number;
  running: number;
}

/** Counts what lies under a node, itself not included, among the rows of a tree; deleted rows are not counted. */
export function subtreeCounts(rows: ReadonlyMap<string, TreeNode>, id: string): SubtreeCounts {
  const counts: SubtreeCounts = { orgs: 0, projects: 0, folders: 0, topics: 0, threads: 0, running: 0 };
  for (const row of descendantsOf(rows, id)) {
    if (row.kind === 'org') counts.orgs += 1;
    else if (row.kind === 'project') {
      if (rows.get(row.parent_id ?? '')?.kind === 'project') counts.folders += 1;
      else counts.projects += 1;
    } else if (row.kind === 'topic') counts.topics += 1;
    else {
      counts.threads += 1;
      if (row.thread?.state === 'running' || row.thread?.state === 'starting') counts.running += 1;
    }
  }
  return counts;
}

/** What a card says of a place: how much work is open in it, and what of it needs a look. */
export interface PlaceStats {
  /** The topics that lie directly in it and are open and not archived: a project's own, and not those of its folders. */
  openTopics: number;
  /** The folders that lie directly in it and are not archived. */
  folders: number;
  /** Threads with a turn running. */
  running: number;
  /** Threads waiting for an approval, and how many approvals wait in all. */
  waiting: number;
  approvals: number;
  /** Threads that cannot go on. */
  blocked: number;
  /** When something last happened in a thread under it. */
  lastActivity: string | null;
}

/**
 * Reads what a card says of a place from the tree that holds it; archived things do not count. Its figures of
 * topics and folders are its own; what needs a look, and when something last happened, take in everything in it.
 */
export function placeStats(rows: ReadonlyMap<string, TreeNode>, id: string): PlaceStats {
  const stats: PlaceStats = { openTopics: 0, folders: 0, running: 0, waiting: 0, approvals: 0, blocked: 0, lastActivity: null };
  for (const row of descendantsOf(rows, id)) {
    if (row.archived) continue;
    if (row.parent_id === id) {
      if (row.kind === 'topic' && row.topic?.status !== 'done') stats.openTopics += 1;
      if (row.kind === 'project') stats.folders += 1;
    }
    if (row.kind !== 'thread' || row.thread === null) continue;
    const { state, pending_approvals: approvals, last_activity_at: at } = row.thread;
    if (state === 'running' || state === 'starting') stats.running += 1;
    if (state === 'waiting_approval') stats.waiting += 1;
    if (state === 'blocked') stats.blocked += 1;
    stats.approvals += approvals;
    if (at !== null && (stats.lastActivity === null || at > stats.lastActivity)) stats.lastActivity = at;
  }
  return stats;
}

/** What the trees held can say of a place that something else only names by its id. */
export interface PlaceLookup {
  /** Whether it is a project that lies in a project. */
  isFolder: (id: string) => boolean;
  /** The places between `above` and it, outermost first, neither of them included; empty when it lies in `above` itself, or when nothing is known. */
  between: (id: string, above: string) => string[];
  /** The way down from the root org to it, itself last, as references; empty when no tree held knows it. */
  pathTo: (id: string) => NodeRef[];
}

/** Reads folders and places on the way from the trees that are held, for texts that name a place by its id alone. */
export function usePlaceLookup(): PlaceLookup {
  const byOrg = useTreeStore((state) => state.byOrg);
  return useMemo(() => {
    const rowOf = (id: string): { row: TreeNode; tree: OrgTree } | null => {
      for (const tree of byOrg.values()) {
        const row = tree.nodes.get(id);
        if (row !== undefined) return { row, tree };
      }
      return null;
    };
    return {
      isFolder: (id) => {
        const found = rowOf(id);
        return found !== null && found.row.kind === 'project' && found.tree.nodes.get(found.row.parent_id ?? '')?.kind === 'project';
      },
      pathTo: (id) => {
        const found = rowOf(id);
        if (found === null) return [];
        return pathTo(found.tree, id).map((entry): NodeRef => ({ id: entry.id, kind: entry.kind, name: entry.name, redacted: false }));
      },
      between: (id, above) => {
        const found = rowOf(id);
        if (found === null) return [];
        const path = pathTo(found.tree, id).slice(0, -1);
        const from = path.findIndex((entry) => entry.id === above);
        return from < 0 ? [] : path.slice(from + 1).map((entry) => entry.name);
      },
    };
  }, [byOrg]);
}
