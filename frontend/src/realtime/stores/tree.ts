/** Tree live state; the socket client owns live updates.
 *
 * The nodes of each root org's tree that the reader may view, as the live feed keeps them: orgs
 * inside the org, projects, topics and threads, each a row with the sequence at which it last
 * changed. Open branches own their pages. A closed branch keeps its own folded row and releases
 * its descendants; native page replacements also remove moved or revoked rows.
 *
 * Components read through the hooks at the end. The writers are for the live connection alone.
 */
import { create } from 'zustand';
import { useStoreWithEqualityFn } from 'zustand/traditional';

import { currentThreadDisplay } from '@/lib/threadDisplay';
import type { HiddenItem, TreeNode, ThreadDisplay } from '@/generated/api-types';

export interface TreeBranch {
  rows: ReadonlySet<string>;
  hidden: ReadonlySet<string>;
  cursor: string | null;
  ancestors?: ReadonlySet<string>;
  pending?: boolean;
  error?: unknown;
}

export interface OrgTree {
  /** Default-open folders belong to the initial small-tree cut, never to later creations. */
  initialOpen?:ReadonlySet<string>|null;
  openingSeq?:number;
  /** Only the pages retained by open branches; absent in the saved shell shape. */
  branches?: ReadonlyMap<string, TreeBranch>;

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
  /** Whether the first visible level was loaded at least once. */
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
  /** Replace one open level. Descendants belong to their own explicit branch readers. */
  branch(orgId: string, parent: string, rows: readonly TreeNode[], seq: number, minimal: readonly HiddenItem[] = [], ancestors: readonly TreeNode[] = [], cursor: string | null = null,pageSize?:number): void {
    update(orgId, (tree) => {
      const branches = new Map(tree.branches);
      const old = branches.get(parent);
      branches.set(parent, { rows: new Set(rows.map((row) => row.id)), hidden: new Set(minimal.map((row) => row.id)), ancestors: new Set(ancestors.map((row)=>row.id)), cursor });
      const nodes = new Map(tree.nodes), hidden = new Map(tree.hidden);
      for (const row of [...ancestors, ...rows]) {
        const display = currentThreadDisplay(row.thread?.display, nodes.get(row.id)?.thread?.display);
        const folded=row.folded??nodes.get(row.id)?.folded;
        const retained=folded===undefined?row:{...row,folded};
        nodes.set(row.id, row.thread === null || display === undefined ? retained : { ...retained, thread: { ...row.thread, display } });
        hidden.delete(row.id);
      }
      for (const row of minimal) { hidden.set(row.id,row); nodes.delete(row.id); }
      const retained = new Set([...branches.values()].flatMap((branch) => [...branch.rows, ...branch.hidden, ...branch.ancestors ?? []]));
      for (const id of old?.rows ?? []) if (!retained.has(id)) {
        nodes.delete(id);
        branches.delete(id);
      }
      for (const id of old?.hidden ?? []) if (!retained.has(id)) hidden.delete(id);
      for (const id of nodes.keys()) if (!retained.has(id)) nodes.delete(id);
      for (const id of hidden.keys()) if (!retained.has(id)) hidden.delete(id);
      // A retained child must still have its parent on a retained path. A moved branch loses its old descendants.
      let changed = true;
      while (changed) {
        changed = false;
        for (const [id,row] of nodes) if (id !== orgId && row.parent_id !== null && !nodes.has(row.parent_id) && !hidden.has(row.parent_id)) {
          nodes.delete(id); branches.delete(id); changed = true;
        }
      }
      const openingSeq=tree.openingSeq??seq;
      let initialOpen=tree.initialOpen;
      if(initialOpen===undefined && parent===orgId) {
        const folded = nodes.get(orgId)?.folded;
        const total=folded?.descendants === undefined ? undefined : folded.descendants + (folded.archived?.descendants ?? 0);
        initialOpen=total!==undefined && pageSize!==undefined && total<=pageSize?new Set<string>():null;
      }
      if(initialOpen!==undefined && initialOpen!==null) {
        const defaults=new Set(initialOpen);
        for(const row of nodes.values())if(row.tree_seq<=openingSeq && (row.kind==='org'||row.kind==='project'))defaults.add(row.id);
        initialOpen=defaults;
      }
      return { ...tree, branches, nodes, hidden,openingSeq,...(initialOpen===undefined?{}:{initialOpen}), lastSeq: Math.max(tree.lastSeq,seq), loaded: true, error: null };
    });
  },

  /** Pending and failed parts keep their own line, while previously read rows remain visible. */
  branchState(orgId:string,parent:string,pending:boolean,error:unknown):void {
    update(orgId,(tree)=>{
      const branches=new Map(tree.branches),held=branches.get(parent) ?? {rows:new Set<string>(),hidden:new Set<string>(),cursor:null};
      branches.set(parent,{...held,pending,error});
      return {...tree,branches,...(parent===orgId?{error}: {})};
    });
  },

  /** A search page owns no collapsed descendants and can be released independently. */
  forgetBranch(orgId:string,parent:string):void {
    update(orgId,(tree)=>{
      const branches=new Map(tree.branches);branches.delete(parent);
      const ids=new Set([...branches.values()].flatMap((branch)=>[...branch.rows,...branch.hidden,...branch.ancestors ?? []]));ids.add(orgId);
      return {...tree,branches,nodes:new Map([...tree.nodes].filter(([id])=>ids.has(id))),hidden:new Map([...tree.hidden ?? []].filter(([id])=>ids.has(id)))};
    });
  },

  /** An address contributes only its own ancestry, never every sibling along the way. */
  reveal(orgId: string, rows: readonly TreeNode[], seq: number): void {
    update(orgId, (tree) => {
      const branches=new Map(tree.branches);branches.set('path',{rows:new Set(rows.map((row)=>row.id)),hidden:new Set(),cursor:null});
      const retained=new Set([...branches.values()].flatMap((branch)=>[...branch.rows,...branch.ancestors ?? []]));retained.add(orgId);
      const nodes=new Map([...tree.nodes,...rows.map((row)=>[row.id,row] as const)].filter(([id])=>retained.has(id)));
      return {...tree,branches,nodes,lastSeq:Math.max(tree.lastSeq,seq)};
    });
  },

  /** Closing a branch drops every child page and descendant row immediately. */
  releaseBranch(orgId: string, parent: string): void {
    update(orgId, (tree) => {
      const remove = new Set<string>();
      for (const id of [...tree.nodes.keys(), ...tree.hidden?.keys() ?? []]) {
        let at: string | null | undefined = id;
        const seen = new Set<string>();
        while (at !== null && at !== undefined && !seen.has(at)) {
          if (at === parent) { if (id !== parent) remove.add(id); break; }
          seen.add(at); at = tree.nodes.get(at)?.parent_id ?? tree.hidden?.get(at)?.parent_id;
        }
      }
      const nodes = new Map([...tree.nodes].filter(([id]) => !remove.has(id)));
      const hidden = new Map([...tree.hidden ?? []].filter(([id]) => !remove.has(id)));
      const branches = new Map([...tree.branches ?? []].filter(([id]) => id !== parent && !remove.has(id)));
      return { ...tree, nodes, hidden, branches };
    });
  },

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
  return useStoreWithEqualityFn(useTreeStore, (state) => findPath(state.byOrg, nodeId ?? null), samePath);
}

/** Every native row of this path stays current; an unrelated thread cannot redraw its readers. */
function samePath(before: NodePath, after: NodePath): boolean {
  return before.orgId === after.orgId && before.path.length === after.path.length
    && before.path.every((node, index) => node === after.path[index]);
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
