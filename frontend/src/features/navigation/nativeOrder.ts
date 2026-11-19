/** Combine safe partial sibling orders with the native page prefix, without exposing saved ranks. */
import type { TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';

/** Prepare once per layout, so separate open levels never rescan one another's rows. */
export function nativeOrder(tree: OrgTree) {
  const info = tree.ordering;
  const branches = [...tree.branches?.values() ?? []].filter((branch) =>
    branch.ordering?.some((order) => info === undefined || order.revision === info.revision));
  const sources = branches.flatMap((branch) => branch.ordering ?? []).filter((order) =>
    info === undefined || order.revision === info.revision);
  const chains = new Map<string, (readonly string[])[]>();
  const remember = (parent: string, ids: readonly string[]) => {
    const held = chains.get(parent) ?? [];
    held.push(ids);
    chains.set(parent, held);
  };
  if (info?.by === 'custom') {
    for (const branch of branches) {
      const parents = new Map<string, string[]>();
      for (const id of branch.rows) {
        const parent = tree.nodes.get(id)?.parent_id;
        if (parent === null || parent === undefined) continue;
        const ids = parents.get(parent) ?? [];
        ids.push(id);
        parents.set(parent, ids);
      }
      for (const [parent, ids] of parents) remember(parent, ids);
    }
    for (const source of sources) for (const [parent, ids] of Object.entries(source.known_order ?? {})) remember(parent, ids);
  }
  return (parent: string, nodes: readonly TreeNode[], fallback: (one: TreeNode, other: TreeNode) => number): TreeNode[] | null => {
    if (sources.length === 0) return null;
    const byId = new Map(nodes.map((node) => [node.id, node]));
    const direct = tree.branches?.get(parent);
    const prefix = [...direct?.rows ?? []].filter((id) => byId.has(id));
    const prefixAndRest = () => {
      const first = new Set(prefix);
      return [
        ...prefix.flatMap((id) => { const node = byId.get(id); return node ? [node] : []; }),
        ...nodes.filter((node) => !first.has(node.id)).sort(fallback),
      ];
    };
    if (info?.by !== 'custom') return prefixAndRest();

    // A chain is native relative order of admitted identities, never private preference positions.
    const edges = new Map(nodes.map((node) => [node.id, new Set<string>()]));
    const degree = new Map(nodes.map((node) => [node.id, 0]));
    const known = new Set<string>();
    const edge = (one: string, other: string) => {
      if (one === other || !byId.has(one) || !byId.has(other) || edges.get(one)?.has(other)) return;
      edges.get(one)?.add(other);
      degree.set(other, (degree.get(other) ?? 0) + 1);
    };
    const chain = (ids: readonly string[]) => {
      const selected = ids.filter((id) => byId.has(id));
      for (const id of selected) known.add(id);
      for (let at = 1; at < selected.length; at += 1) {
        const before = selected[at - 1], after = selected[at];
        if (before !== undefined && after !== undefined) edge(before, after);
      }
    };
    for (const ids of chains.get(parent) ?? []) chain(ids);
    const last = prefix.at(-1), inPrefix = new Set(prefix);
    if (last !== undefined) for (const node of nodes) if (!inPrefix.has(node.id)) edge(last, node.id);

    // A heap keeps independent ready chains in familiar fallback order with O(n log n) work.
    const compare = (one: TreeNode, other: TreeNode) =>
      Number(known.has(other.id)) - Number(known.has(one.id)) || fallback(one, other);
    const ready: TreeNode[] = [];
    const push = (node: TreeNode) => {
      ready.push(node);
      let at = ready.length - 1;
      while (at > 0) {
        const parent = Math.floor((at - 1) / 2), before = ready[parent];
        if (before === undefined || compare(before, node) <= 0) break;
        ready[at] = before;
        at = parent;
      }
      ready[at] = node;
    };
    const pop = (): TreeNode | undefined => {
      const first = ready[0], last = ready.pop();
      if (ready.length === 0 || last === undefined) return first;
      let at = 0;
      for (;;) {
        let child = at * 2 + 1;
        if (child >= ready.length) break;
        const one = ready[child], other = ready[child + 1];
        if (other !== undefined && one !== undefined && compare(other, one) < 0) child += 1;
        const next = ready[child];
        if (next === undefined || compare(last, next) <= 0) break;
        ready[at] = next;
        at = child;
      }
      ready[at] = last;
      return first;
    };
    for (const node of nodes) if (degree.get(node.id) === 0) push(node);
    const ordered: TreeNode[] = [];
    while (ready.length > 0) {
      const node = pop();
      if (node === undefined) break;
      ordered.push(node);
      for (const id of edges.get(node.id) ?? []) {
        const remaining = (degree.get(id) ?? 0) - 1;
        degree.set(id, remaining);
        const next = byId.get(id);
        if (remaining === 0 && next !== undefined) push(next);
      }
    }
    // Concurrent snapshots may disagree briefly; retain the direct prefix until its replacement arrives.
    return ordered.length === nodes.length ? ordered : prefixAndRest();
  };
}
