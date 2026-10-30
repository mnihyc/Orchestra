/** Where the tree of the left bar begins and which of its rows are open: what one device keeps of a tree, the place the tree is narrowed to, and the branches that lead to a row.
 *
 * The tree begins at the root org unless its reader narrowed it to a place inside: an org, a project,
 * a folder or a topic. Other tools call this focusing, zooming, hoisting or changing the root. It is
 * the reader's own act and stays where they put it; going elsewhere does not move it. A place that is
 * gone, or no longer theirs to see, narrows nothing: the tree begins at the org again.
 *
 * Both the beginning and the open rows belong to the work under way on one device: they are kept
 * there, one of each for every org, read when a page loads, and never pushed into a page that is open.
 *
 * Folding away works on the rows that are open: the branches that lead to a row stay as they are, and
 * every other one closes. Nothing folds by itself.
 */
import { create } from 'zustand';

import type { TreeNode } from '@/generated/api-types';
import { type OrgTree, pathTo } from '@/realtime/stores/tree';

// ---------------------------------------------------------------- what the device keeps

const openKey = (userId: string, orgId: string) => `orch.${userId}.tree.${orgId}`;
const beginningKey = (userId: string, orgId: string) => `orch.${userId}.tree.${orgId}.begins`;

/** The rows a person left open in the tree of an org, on this device; `null` where it keeps none. */
export function readOpen(userId: string | null, orgId: string): Set<string> | null {
  if (userId === null) return null;
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(openKey(userId, orgId)) ?? 'null');
    return Array.isArray(stored) ? new Set(stored.filter((id): id is string => typeof id === 'string')) : null;
  } catch {
    return null;
  }
}

export function writeOpen(userId: string | null, orgId: string, open: ReadonlySet<string>): void {
  if (userId === null) return;
  try {
    localStorage.setItem(openKey(userId, orgId), JSON.stringify([...open]));
  } catch {
    // A device that keeps nothing still shows the tree; it only opens the same rows next time.
  }
}

/** The place a person narrowed the tree of an org to, on this device; `null` where the tree shows everything. */
export function readBeginning(userId: string | null, orgId: string): string | null {
  if (userId === null) return null;
  try {
    const stored = localStorage.getItem(beginningKey(userId, orgId));
    return stored === null || stored === '' ? null : stored;
  } catch {
    return null;
  }
}

export function writeBeginning(userId: string | null, orgId: string, nodeId: string | null): void {
  if (userId === null) return;
  try {
    if (nodeId === null) localStorage.removeItem(beginningKey(userId, orgId));
    else localStorage.setItem(beginningKey(userId, orgId), nodeId);
  } catch {
    // A device that keeps nothing shows everything next time.
  }
}

// ---------------------------------------------------------------- the place a tree begins at

/** The kinds of place a tree can begin at: a thread holds nothing a tree could show. */
const HOLDS_A_TREE: ReadonlySet<TreeNode['kind']> = new Set(['org', 'project', 'topic']);

const whole = (tree: Pick<OrgTree, 'nodes'>): OrgTree => ({ nodes: tree.nodes, lastSeq: 0, loaded: true, error: null });

/** The places from the root org down to a node, itself last. */
export function placesTo(tree: Pick<OrgTree, 'nodes'>, nodeId: string): TreeNode[] {
  return pathTo(whole(tree), nodeId);
}

/** The places from the root org down to a node, itself last, by id. */
export function wayOf(tree: Pick<OrgTree, 'nodes'>, nodeId: string): string[] {
  return placesTo(tree, nodeId).map((node) => node.id);
}

/** Whether a node is the place, or lies in it. */
export function liesIn(tree: Pick<OrgTree, 'nodes'>, nodeId: string, placeId: string): boolean {
  return wayOf(tree, nodeId).includes(placeId);
}

/** Whether the tree can be narrowed to a row: a place inside the root org that is still in view. */
export function canBeginAt(tree: Pick<OrgTree, 'nodes'>, orgId: string, nodeId: string, showArchived = false): boolean {
  const node = tree.nodes.get(nodeId);
  if (node === undefined || node.deleted || nodeId === orgId || !HOLDS_A_TREE.has(node.kind)) return false;
  // Every place on the way must be there: a place under one that is gone, archived out of view or hidden is not reached from the root.
  const way = placesTo(tree, nodeId);
  return way[0]?.id === orgId && way.every((place) => !place.deleted && (showArchived || !place.archived));
}

/** The place the tree begins at: the one that was asked for where it still can be, else `null`, which is the root org and shows everything. */
export function beginningOf(tree: Pick<OrgTree, 'nodes'>, orgId: string, wanted: string | null, showArchived = false): string | null {
  return wanted !== null && canBeginAt(tree, orgId, wanted, showArchived) ? wanted : null;
}

/**
 * Where "Show only where I am" leads: the place of the screen in view, which is that place itself where it
 * is an org inside the root, a project, a folder or a topic, and the topic of a thread. A document is
 * where the node it was written beside is. `null` where that is the root org, where the tree begins there
 * already, or where the screen is of nothing in this tree.
 */
export function whereIAm(tree: Pick<OrgTree, 'nodes'>, orgId: string, nodeId: string | null, beginningId: string | null, showArchived = false): string | null {
  if (nodeId === null) return null;
  const place = beginningFor(tree, orgId, nodeId, showArchived);
  return place === beginningId ? null : place;
}

/**
 * The project a thing is of: the outermost project it lies in, which is no folder, or the thing itself
 * where it is that project; where no project is on the way, the nearest org inside the root. `null`
 * where the thing is the root org's own, which only the whole tree shows.
 */
export function projectOf(tree: Pick<OrgTree, 'nodes'>, orgId: string, nodeId: string): string | null {
  const way = placesTo(tree, nodeId);
  if (way[0]?.id !== orgId) return null;
  const project = way.find((place) => place.kind === 'project');
  if (project !== undefined) return project.id;
  for (let at = way.length - 1; at >= 1; at -= 1) {
    const place = way[at];
    if (place?.kind === 'org') return place.id;
  }
  return null;
}

/** The place one step above where the tree begins; `null` where that is the root org, which shows everything. */
export function placeAbove(tree: Pick<OrgTree, 'nodes'>, orgId: string, beginningId: string): string | null {
  const above = tree.nodes.get(beginningId)?.parent_id ?? null;
  return above === null || above === orgId ? null : above;
}

/** The place a row narrows the tree to: the row itself where a tree can begin at it, else the nearest place above it that one can. */
export function beginningFor(tree: Pick<OrgTree, 'nodes'>, orgId: string, nodeId: string, showArchived = false): string | null {
  const way = wayOf(tree, nodeId);
  for (let at = way.length - 1; at >= 1; at -= 1) {
    const place = way[at];
    if (place !== undefined && canBeginAt(tree, orgId, place, showArchived)) return place;
  }
  return null;
}

// ---------------------------------------------------------------- folding away

/** The rows that stay open when everything that does not lead to a row is folded: those above it, and itself with what is open inside it. */
export function othersFolded(tree: Pick<OrgTree, 'nodes'>, expanded: ReadonlySet<string>, nodeId: string): Set<string> {
  const above = new Set(wayOf(tree, nodeId));
  const kept = new Set<string>();
  for (const id of expanded) {
    // A row that is no node (the row of the org's group chats) leads to no place.
    if (!tree.nodes.has(id)) continue;
    if (above.has(id) || liesIn(tree, id, nodeId)) kept.add(id);
  }
  // The way down to the row is open, whatever it was.
  for (const id of above) if (id !== nodeId) kept.add(id);
  return kept;
}

/** The rows that are open once the way down to a node is: what was open, and every place above it. `itself` opens the node too. */
export function wayOpened(tree: Pick<OrgTree, 'nodes'>, expanded: ReadonlySet<string>, nodeId: string, itself = false): Set<string> {
  const way = wayOf(tree, nodeId);
  return new Set([...expanded, ...(itself ? way : way.slice(0, -1))]);
}

// ---------------------------------------------------------------- what a row's menu asks of the bar

/** What a row's menu can ask of the tree that is drawn: the bar gives these while it is shown. */
export interface TreeActs {
  /** Whether the tree can be narrowed to the row. */
  canShowOnly: (nodeId: string) => boolean;
  /** Narrows the tree to the row. */
  showOnly: (nodeId: string) => void;
  /** Folds every branch that does not lead to the row. */
  collapseOthers: (nodeId: string) => void;
  /** Whether anything in the row, or the row itself, is unread for the reader or mentions them. */
  hasUnread: (nodeId: string) => boolean;
  /** Says that the reader has read every thread and every chat of the row and of what lies in it. */
  markRead: (nodeId: string) => void;
}

/** The acts of the tree that is drawn, or `null` while no bar is shown. */
export const useTreeActs = create<{ acts: TreeActs | null }>(() => ({ acts: null }));
