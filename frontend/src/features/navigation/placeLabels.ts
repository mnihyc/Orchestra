/** The places the reader can see, as the labels of places tell them apart, and the hooks every surface that names a place labels it with.
 *
 * The places a reader can see are those of the root orgs they are in (`GET /orgs`) and of the trees of those
 * orgs: the trees the page follows live and those already read by an explicitly opened place picker.
 * Labeling alone never fetches other trees. A list adds its own rows (search hits, the picker's places, Inbox rows each
 * say where they are). The label itself is `src/lib/placeLabel.ts`: the name alone, or with where it is when the
 * name is shared ("dep2 · Acme" against "dep2 · Beta").
 */
import { useQueries, useQuery } from '@tanstack/react-query';
import { useCallback, useMemo } from 'react';

import { fetchTree, orgsQuery } from '@/api/nodes';
import type { NodeKind, TreeNode } from '@/generated/api-types';
import { type KnownPlaces, knownPlaces, type Placed, placeLabel, placeLabels } from '@/lib/placeLabel';
import { type OrgTree, pathTo, useTreeStore } from '@/realtime/stores/tree';

/** The kinds of node that are places: what a person names, finds and is given access to by where it lies. */
const PLACE_KINDS: ReadonlySet<NodeKind> = new Set<NodeKind>(['org', 'project', 'topic']);

/**
 * How many of the reader's orgs have their trees read for labels besides the trees the page follows: the orgs the
 * rail shows without scrolling on a laptop's screen. Somebody in more orgs than that is an administrator of many,
 * whose trees together would be a large read at every start; the lists they open still tell their own rows apart.
 */
const TREES_READ_FOR_LABELS = 20;

/** How long a tree read for labels is kept before it is read again: names seldom change, and an act that changes one asks again at once. */
const LABEL_TREES_FRESH_MS = 5 * 60_000;

/** The places of one tree, each with the way to it; worked out once for each state of the tree. */
const placesOfTree = new WeakMap<OrgTree, Placed[]>();

function placesIn(tree: OrgTree): Placed[] {
  const held = placesOfTree.get(tree);
  if (held !== undefined) return held;
  const found: Placed[] = [];
  for (const node of tree.nodes.values()) {
    if (node.deleted || !PLACE_KINDS.has(node.kind)) continue;
    found.push(placedOfRow(tree, node));
  }
  placesOfTree.set(tree, found);
  return found;
}

/** A row of a tree as its label needs it. */
export function placedOfRow(tree: OrgTree, node: TreeNode): Placed {
  return { id: node.id, name: node.name, above: pathTo(tree, node.id).slice(0, -1).map((one) => ({ id: one.id, name: one.name })), address: node.address };
}

/** The rows of each tree read, as one value: a function that stays the same, so that the value stays the same object while no tree changed. */
const rowsOfTrees = (results: readonly { data?: { nodes: TreeNode[] } | undefined }[]): (TreeNode[] | undefined)[] => results.map((result) => result.data?.nodes);

/** Rows of a tree that was read, as a tree the label can walk. */
const asTree = (rows: readonly TreeNode[]): OrgTree => ({ nodes: new Map(rows.map((row) => [row.id, row])), lastSeq: 0, loaded: true, error: null });

/**
 * The places the reader can see, as far as the page knows them. `trees: false` asks for the root orgs alone (the
 * rail, the org switcher), whose names the list of orgs tells apart by their addresses.
 */
export function useKnownPlaces({ trees = true, loadTrees = false }: { trees?: boolean; loadTrees?: boolean } = {}): KnownPlaces {
  const byOrg = useTreeStore((state) => state.byOrg);
  // The orgs as whatever asked for them read them: this asks nothing itself.
  const orgs = useQuery({ ...orgsQuery, enabled: false }).data;
  const others = useQueries({
    queries: (trees && loadTrees ? (orgs ?? []) : [])
      .filter((org) => !byOrg.has(org.id))
      .slice(0, TREES_READ_FOR_LABELS)
      .map((org) => ({
        queryKey: ['tree-places', org.id] as const,
        queryFn: ({ signal }: { signal: AbortSignal }) => fetchTree(org.id, null, signal),
        staleTime: LABEL_TREES_FRESH_MS,
      })),
    combine: rowsOfTrees,
  });
  const read = useMemo(() => others.filter((rows): rows is TreeNode[] => rows !== undefined), [others]);
  return useMemo(() => {
    const places: Placed[] = [];
    for (const org of orgs ?? []) places.push({ id: org.id, name: org.name, above: [], address: org.address });
    if (trees) {
      for (const tree of byOrg.values()) places.push(...placesIn(tree));
      for (const rows of read) {
        const tree = asTree(rows);
        for (const node of rows) if (!node.deleted && PLACE_KINDS.has(node.kind)) places.push(placedOfRow(tree, node));
      }
    }
    return knownPlaces(places);
  }, [byOrg, orgs, read, trees]);
}

/** The labels of the places of a list, told apart from each other and from every place the reader can see. `trees: false` for a list of root orgs. */
export function usePlaceLabels(places: readonly Placed[], options: { trees?: boolean; loadTrees?: boolean } = {}): ReadonlyMap<string, string> {
  const known = useKnownPlaces(options);
  return useMemo(() => placeLabels(places, known), [places, known]);
}

/** A readable reference gets the same label as a list; a redacted reference never reads a cached name. */
export function usePlaceLabel(): (reference: {id:string;name:string|null;redacted?:boolean} | null | undefined) => string | null {
  const known = useKnownPlaces();
  return useCallback((reference) => {
    if (reference?.name === undefined || reference.name === null || reference.redacted === true) return null;
    const held = [...known.values()].flat().find((place) => place.id === reference.id);
    return placeLabel(held === undefined ? {id:reference.id,name:reference.name,above:[]} : {...held,name:reference.name},known);
  },[known]);
}

/** The label of one place, by its id: from the trees the page knows; `fallback` where none holds it, or where it is no place's own name. */
export function useNodeLabel(nodeId: string | null | undefined, fallback: string | undefined): string | undefined {
  const byOrg = useTreeStore((state) => state.byOrg);
  const known = useKnownPlaces({ trees: nodeId !== null && nodeId !== undefined });
  return useMemo(() => {
    if (nodeId === null || nodeId === undefined || fallback === undefined) return fallback;
    for (const tree of byOrg.values()) {
      const node = tree.nodes.get(nodeId);
      // A screen that names something else under a place (its settings, its usage) keeps its own words.
      if (node === undefined || node.deleted || node.name.trim() !== fallback.trim()) continue;
      if (node.kind === 'thread') {
        const threads = [...byOrg.values()].flatMap((held) => [...held.nodes.values()].filter((row) => row.kind === 'thread' && !row.deleted).map((row) => placedOfRow(held, row)));
        return placeLabels([placedOfRow(tree, node)], knownPlaces(threads)).get(node.id) ?? fallback;
      }
      if (!PLACE_KINDS.has(node.kind)) continue;
      return placeLabels([placedOfRow(tree, node)], known).get(node.id) ?? fallback;
    }
    return fallback;
  }, [byOrg, known, nodeId, fallback]);
}
