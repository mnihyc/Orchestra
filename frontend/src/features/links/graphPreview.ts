/** Keep the inspector's small graph readable; the full graph retains every returned node. */
import type { GraphNode, LinkGraph } from '@/generated/api-types';

/** Four neighbours leave room for their names around the centre in the 280px inspector canvas. */
export const PREVIEW_NEIGHBOURS = 4;

export function graphPreview(graph: LinkGraph): LinkGraph {
  if (graph.nodes.length <= PREVIEW_NEIGHBOURS + 1) return graph;
  const center = graph.nodes.find((node) => node.id === graph.center) ?? graph.nodes[0];
  if (center === undefined) return graph;
  const nearby: GraphNode[] = [];
  for (const node of graph.nodes) {
    if (node.id === center.id) continue;
    nearby.push(node);
    nearby.sort((one, other) => one.distance - other.distance || one.name.localeCompare(other.name) || one.id.localeCompare(other.id));
    if (nearby.length > PREVIEW_NEIGHBOURS) nearby.pop();
  }
  const nodes = [center, ...nearby];
  const kept = new Set(nodes.map((node) => node.id));
  return { ...graph, nodes, edges: graph.edges.filter((edge) => kept.has(edge.from) && kept.has(edge.to)), truncated: true };
}
