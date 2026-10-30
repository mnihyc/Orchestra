/** Finder names and search hits use the common place label with their own readable paths as peers. */
import type { NameHit, NodeRef, SearchHit } from '@/generated/api-types';
import { foldName, type KnownPlaces, type Placed, placeLabels } from '@/lib/placeLabel';

function pathPlaces(path: readonly NodeRef[], known: KnownPlaces, found: Map<string, Placed>): void {
  const complete = path[0]?.kind === 'org';
  const safe = path.map((node) => ({ id: node.id, name: node.redacted ? null : node.name }));
  for (const [index, node] of path.entries()) {
    if (node.redacted || node.name === null) continue;
    const peer = known.get(foldName(node.name))?.find((place) => place.id === node.id);
    // Item hits begin at the project. A held readable path knows its orgs too; a complete
    // current path takes precedence and suppresses every name its reader may not know.
    const above = !complete && peer !== undefined ? peer.above.map((one) => {
      return safe.find((ref) => ref.id === one.id) ?? one;
    }) : safe.slice(0, index);
    const previous = found.get(node.id);
    if (previous === undefined || complete || previous.above.length < above.length) found.set(node.id, { id: node.id, name: node.name, above, address: peer?.address });
  }
}

/** The place names of all visible name groups share one comparison, including the list's own peers. */
export function labelsOfNames(hits: readonly NameHit[], known: KnownPlaces = new Map()): ReadonlyMap<string, string> {
  const places = new Map<string, Placed>();
  for (const hit of hits) {
    if (hit.kind === 'org' || hit.kind === 'project' || hit.kind === 'topic' || hit.kind === 'thread') {
      const above = hit.path.at(-1)?.id === hit.id ? hit.path.slice(0, -1) : hit.path;
      pathPlaces([...above, { id: hit.id, name: hit.name, kind: hit.kind, redacted: false }], known, places);
    } else pathPlaces(hit.path, known, places);
    if (hit.kind === 'org') {
      const place = places.get(hit.id);
      if (place !== undefined) places.set(hit.id, { ...place, address: hit.address });
    }
  }
  return placeLabels([...places.values()], known);
}

/** A name keeps its identity and route; a node chat names its node with that same label. */
export function labelOfName(hit: NameHit, labels: ReadonlyMap<string, string>): string {
  if (hit.kind === 'org' || hit.kind === 'project' || hit.kind === 'topic' || hit.kind === 'thread') return labels.get(hit.id) ?? hit.name;
  const node = hit.kind === 'chat' ? hit.path.at(-1) : undefined;
  return node === undefined || node.redacted ? hit.name : labels.get(node.id) ?? hit.name;
}

/** Every readable place in the result page contributes to the shared labels, including duplicate names in different orgs. */
export function labelsOfHits(hits: readonly SearchHit[], known: KnownPlaces = new Map()): ReadonlyMap<string, string> {
  const places = new Map<string, Placed>();
  for (const hit of hits) pathPlaces(hit.kind === 'item' ? [hit.project, hit.topic, hit.thread] : hit.path, known, places);
  return placeLabels([...places.values()], known);
}

/** A path supplies its own ancestors; a partial path needs at most one of the common labels' qualifiers. */
export function labelsAlongPath(path: readonly NodeRef[], labels: ReadonlyMap<string, string>, privateName: (kind: NodeRef['kind']) => string): string[] {
  let qualified = path[0]?.kind === 'org';
  return path.map((place) => {
    if (place.redacted || place.name === null) return privateName(place.kind);
    const label = labels.get(place.id) ?? place.name;
    if (!qualified && label !== place.name) {
      qualified = true;
      return label;
    }
    return place.name;
  });
}
