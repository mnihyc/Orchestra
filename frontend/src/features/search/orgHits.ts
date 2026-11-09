/** Root names come from authorized hit paths, live trees or the hit's ordinary readable node. */
import { useQueries } from '@tanstack/react-query';
import { nodeQuery } from '@/api/nodes';
import { keyOfHit, wayOfHit } from '@/features/search/hits';
import type { Node, NodeRef, Org, SearchHit } from '@/generated/api-types';
import { useTreeStore } from '@/realtime/stores/tree';

function containingNode(hit: SearchHit): string {
  return hit.kind === 'item' ? hit.thread.id : hit.kind === 'document' ? hit.document.node.id
    : hit.kind === 'name' ? hit.node.id : hit.chat.node?.id ?? '';
}

export function hitRoot(hit: SearchHit, orgs: readonly Org[], nodes: ReadonlyMap<string, Pick<Node, 'org_id' | 'path'>> = new Map()): NodeRef | null {
  const path = wayOfHit(hit);
  const first = path[0];
  if (first?.kind === 'org') return first;
  const node = nodes.get(containingNode(hit));
  const id = hit.kind === 'chat_message' ? hit.chat.org_id : node?.org_id;
  if (id === undefined) return null;
  const native = node?.path.find((entry) => entry.id === id);
  if (native !== undefined) return native;
  const org = orgs.find((entry) => entry.id === id);
  return { id, kind: 'org', name: org?.name ?? null, redacted: org === undefined };
}

export function useHitOrgs(hits: readonly SearchHit[], orgs: readonly Org[] | undefined, enabled: boolean) {
  const trees = useTreeStore((state) => state.byOrg);
  const known = new Map<string, Pick<Node, 'org_id' | 'path'>>();
  // A tree's key is its root, even when a node's ancestor names are private.
  for (const [id, tree] of trees) for (const node of tree.nodes.values()) {
    if (!node.deleted) known.set(node.id, { org_id: id, path: [] });
  }
  const missing = enabled ? [...new Set(hits.filter((hit) => hitRoot(hit, orgs ?? [], known) === null).map(containingNode).filter((id) => id !== ''))] : [];
  const reads = useQueries({ queries: missing.map((id) => ({ ...nodeQuery(id), retry: false })) });
  reads.forEach((read, index) => {
    const id = missing[index];
    if (id !== undefined && read.data !== undefined) known.set(id, read.data);
  });
  return new Map(hits.map((hit) => [keyOfHit(hit), hitRoot(hit, orgs ?? [], known)] as const));
}

/** A single answering root needs no group heading; within a group the native order stays intact. */
export function groupHitIds(ids: readonly string[], roots: ReadonlyMap<string, NodeRef | null>): { root: NodeRef | null; ids: string[] }[] {
  const distinct = new Set(ids.flatMap((id) => { const root = roots.get(id); return root === undefined || root === null ? [] : [root.id]; }));
  if (distinct.size < 2) return [{ root: null, ids: [...ids] }];
  const groups = new Map<string, { root: NodeRef | null; ids: string[] }>();
  for (const id of ids) {
    const root = roots.get(id) ?? null;
    const key = root?.id ?? '';
    const group = groups.get(key) ?? { root, ids: [] };
    group.ids.push(id); groups.set(key, group);
  }
  return [...groups.values()];
}
