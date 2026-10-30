/** The readable places of one root tree, keeping folder ancestry and eligible topic leaves. */
import type { TreeNodeListedContent } from '@/generated/api-types';
import type { TreeRow } from '@/ui/Tree';

export type MovePlace = Pick<TreeNodeListedContent, 'id' | 'kind' | 'parent_id' | 'name' | 'color' | 'archived' | 'deleted'>;

export function topicTree(nodes: ReadonlyMap<string, MovePlace>, root: string, expanded: ReadonlySet<string>): TreeRow<MovePlace>[] {
  const children = new Map<string, MovePlace[]>();
  for (const node of nodes.values()) {
    if (node.deleted || node.archived || node.kind === 'thread' || node.parent_id === null) continue;
    const list = children.get(node.parent_id) ?? [];
    list.push(node); children.set(node.parent_id, list);
  }
  for (const list of children.values()) list.sort((one, two) => one.name.localeCompare(two.name));
  const result: TreeRow<MovePlace>[] = [];
  const visited = new Set<string>();
  const visit = (node: MovePlace, depth: number) => {
    if (visited.has(node.id) || node.deleted || node.archived) return;
    visited.add(node.id);
    const expandable = node.kind !== 'topic' && (children.get(node.id)?.length ?? 0) > 0;
    result.push({ id: node.id, depth, item: node, parentId: node.parent_id, expandable, expanded: expandable && expanded.has(node.id) });
    if (expandable && expanded.has(node.id)) for (const child of children.get(node.id) ?? []) visit(child, depth + 1);
  };
  const first = nodes.get(root);
  if (first !== undefined) visit(first, 0);
  return result;
}
