/** Focus uses the Frame's explicit assignment/follow fact, independently of readability or admin reach. */
import type { TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
export function participates(value: unknown): boolean {
  if (!record(value)) return false;
  const candidates = [value, value.access, value.can];
  for (const candidate of candidates) {
    if (record(candidate) && typeof candidate.participating === 'boolean') return candidate.participating;
  }
  return false;
}

const focused = new WeakMap<ReadonlyMap<string, TreeNode>, Map<string, TreeNode>>();
/** A matching descendant keeps its actual ancestors, with the same row identities and ordering. */
export function participatingTree(tree: OrgTree): OrgTree {
  let nodes = focused.get(tree.nodes);
  if (nodes === undefined) {
    const keep = new Set<string>();
    for (const node of tree.nodes.values()) {
      if (!participates(node)) continue;
      let current: TreeNode | undefined = node;
      while (current !== undefined && !keep.has(current.id)) {
        keep.add(current.id);
        current = current.parent_id === null ? undefined : tree.nodes.get(current.parent_id);
      }
    }
    nodes = new Map([...tree.nodes].filter(([id]) => keep.has(id)));
    focused.set(tree.nodes, nodes);
  }
  return { ...tree, nodes };
}
