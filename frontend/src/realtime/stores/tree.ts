/** Tree live state; the socket client owns live updates.
 *
 * The nodes of each root org's tree that the reader may view, as the live feed keeps them: orgs
 * inside the org, projects, topics and threads, each a row with the sequence at which it last
 * changed. A row is replaced only by a newer one. Nothing is removed here on the page's own
 * initiative: a node that was deleted, or that the reader may no longer see, arrives as a row that
 * says so, and is then left out of what is read.
 *
 * Components read through the hooks at the end. The writers are for the live connection alone.
 */
import { useMemo } from 'react';
import { create } from 'zustand';

import { currentThreadDisplay } from '@/lib/threadDisplay';
import type { HiddenItem, TreeNode, ThreadDisplay } from '@/generated/api-types';

export interface OrgTree {
  /** Every row the feed delivered, deleted ones included, by id. */
  nodes: ReadonlyMap<string, TreeNode>;
  /**
   * The minimal rows of private places the reader administers and may not view, by id: where each lies,
   * its kind, name and first owner, and nothing else. A place is in one map or the other, never both.
   * Absent from a tree that was put together from other rows than the feed's.
   */
  hidden?: ReadonlyMap<string, HiddenItem>;
  /** The tree sequence the rows are as of. */
  lastSeq: number;
  /** Whether the tree was loaded whole at least once. */
  loaded: boolean;
  /** What the last attempt to load it failed with, while nothing newer has succeeded. */
  error: unknown;
}

interface TreeStore {
  byOrg: ReadonlyMap<string, OrgTree>;
}

const EMPTY: OrgTree = { nodes: new Map(), hidden: new Map(), lastSeq: 0, loaded: false, error: null };

export const useTreeStore = create<TreeStore>(() => ({ byOrg: new Map() }));

function update(orgId: string, change: (tree: OrgTree) => OrgTree): void {
  useTreeStore.setState((state) => {
    const before = state.byOrg.get(orgId) ?? EMPTY;
    const after = change(before);
    if (after === before) return state;
    const byOrg = new Map(state.byOrg);
    byOrg.set(orgId, after);
    return { byOrg };
  });
}

/** The writers. Only the live connection calls them. */
export const treeWriter = {
  /** Applies a personal icon update only to a thread this live tree already authorizes. */
  display(threadId: string, display: ThreadDisplay): void {
    for (const [orgId, tree] of useTreeStore.getState().byOrg) {
      const row = tree.nodes.get(threadId);
      if (!row?.thread || row.deleted || currentThreadDisplay(display, row.thread.display) !== display) continue;
      const thread = row.thread;
      update(orgId, (before) => {
        const nodes = new Map(before.nodes);
        nodes.set(threadId, { ...row, thread: { ...thread, display } });
        return { ...before, nodes };
      });
    }
  },
  /** Takes a whole tree in place of what was held, with the minimal rows of what the reader administers. */
  reset(orgId: string, nodes: readonly TreeNode[], seq: number, hidden: readonly HiddenItem[] = []): void {
    update(orgId, (before) => ({
      nodes: new Map(nodes.map((node) => {
        const display = currentThreadDisplay(node.thread?.display, before.nodes.get(node.id)?.thread?.display);
        return [node.id, node.thread === null || display === undefined ? node : { ...node, thread: { ...node.thread, display } }];
      })),
      hidden: new Map(hidden.map((row) => [row.id, row])),
      lastSeq: seq,
      loaded: true,
      error: null,
    }));
  },

  /**
   * Takes rows that changed: each readable row replaces the row held only when it is newer. A minimal row
   * carries no sequence of its own: it is as of the frame that brought it, so it replaces what is held of
   * that place when the frame is newer than what was read; a readable row of a place takes it out again.
   */
  apply(orgId: string, rows: readonly TreeNode[], seq: number, hidden: readonly HiddenItem[] = []): void {
    update(orgId, (tree) => {
      let nodes: Map<string, TreeNode> | null = null;
      let minimal: Map<string, HiddenItem> | null = null;
      const heldHidden = tree.hidden ?? new Map<string, HiddenItem>();
      for (const row of rows) {
        const held = tree.nodes.get(row.id);
        if (held !== undefined && held.tree_seq >= row.tree_seq) continue;
        nodes ??= new Map(tree.nodes);
        const display = currentThreadDisplay(row.thread?.display, held?.thread?.display);
        nodes.set(row.id, row.thread === null || display === undefined ? row : { ...row, thread: { ...row.thread, display } });
        if (heldHidden.has(row.id)) {
          minimal ??= new Map(heldHidden);
          minimal.delete(row.id);
        }
      }
      if (seq > tree.lastSeq || tree.lastSeq === 0) {
        for (const row of hidden) {
          minimal ??= new Map(heldHidden);
          minimal.set(row.id, row);
          if ((nodes ?? tree.nodes).has(row.id)) {
            nodes ??= new Map(tree.nodes);
            nodes.delete(row.id);
          }
        }
      }
      if (nodes === null && minimal === null && seq <= tree.lastSeq) return tree;
      return { ...tree, nodes: nodes ?? tree.nodes, hidden: minimal ?? heldHidden, lastSeq: Math.max(tree.lastSeq, seq), error: null };
    });
  },

  /** Records that the tree could not be loaded. What is held stays, to be read greyed. */
  failed(orgId: string, error: unknown): void {
    update(orgId, (tree) => ({ ...tree, error }));
  },

  /** Forgets a tree nobody shows any more. */
  evict(orgId: string): void {
    useTreeStore.setState((state) => {
      if (!state.byOrg.has(orgId)) return state;
      const byOrg = new Map(state.byOrg);
      byOrg.delete(orgId);
      return { byOrg };
    });
  },

  /** Forgets everything: whoever was signed in has left. */
  clear(): void {
    useTreeStore.setState({ byOrg: new Map() });
  },
};

// ---------------------------------------------------------------- reading

/** The tree of a root org as far as it has arrived. */
export function useOrgTree(orgId: string | null): OrgTree {
  return useTreeStore((state) => (orgId === null ? EMPTY : (state.byOrg.get(orgId) ?? EMPTY)));
}

/** A node by its id, in whichever tree holds it; `null` while none does, or when it was deleted. */
export function useTreeNode(nodeId: string | null | undefined): TreeNode | null {
  return useTreeStore((state) => {
    if (nodeId === null || nodeId === undefined) return null;
    for (const tree of state.byOrg.values()) {
      const node = tree.nodes.get(nodeId);
      if (node !== undefined) return node.deleted ? null : node;
    }
    return null;
  });
}

/** The nodes from the root org down to a node, the node itself last; empty while the node is not held. */
export function pathTo(tree: OrgTree, nodeId: string): TreeNode[] {
  const path: TreeNode[] = [];
  let at = tree.nodes.get(nodeId);
  // A tree is no deeper than its rows are many; the count guards against a row that names itself above.
  for (let steps = 0; at !== undefined && steps <= tree.nodes.size; steps += 1) {
    path.unshift(at);
    at = at.parent_id === null ? undefined : tree.nodes.get(at.parent_id);
  }
  return path;
}

/** The path to a node, and the root org whose tree holds it. */
export function useNodePath(nodeId: string | null | undefined): NodePath {
  const byOrg = useTreeStore((state) => state.byOrg);
  return useMemo(() => findPath(byOrg, nodeId ?? null), [byOrg, nodeId]);
}

export interface NodePath {
  /** The root org whose tree holds the node. */
  orgId: string | null;
  path: TreeNode[];
}

function findPath(byOrg: ReadonlyMap<string, OrgTree>, nodeId: string | null): NodePath {
  if (nodeId !== null) {
    for (const [orgId, tree] of byOrg) {
      if (tree.nodes.has(nodeId)) return { orgId, path: pathTo(tree, nodeId) };
    }
  }
  return { orgId: null, path: [] };
}
