/** The one act that moves places to the trash, from a menu or from a selection, with its one line after it.
 *
 * What is selected inside another selected place goes with that place and is not asked for again. A screen that was
 * showing something deleted goes to the place above before the tree drops it. The line says how many running threads
 * the act stopped, names whatever was refused, and offers Show (the trash of the place it lay in) and Undo.
 */
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { type PlaceRef, placeOfRow, placePath, subtreeCounts } from '@/features/navigation/places';
import type { NodeDeleted } from '@/generated/api-types';
import { type OrgTree, pathTo, useTreeStore } from '@/realtime/stores/tree';
import { useCurrentHeading } from '@/shell/screen';
import { useErrorText } from '@/ui/errorText';

import { restorePlaces, runningIn, trashPlaces } from './places';
import { sayTrashed } from './tip';
import { placeById } from './where';
import { outermost, type TrashResult } from './trash';

function treeOf(id: string): OrgTree | null {
  for (const tree of useTreeStore.getState().byOrg.values()) if (tree.nodes.has(id)) return tree;
  return null;
}

/** The ids above a place, from its root, as the held tree says; empty where no tree holds it. */
function above(id: string): string[] {
  const tree = treeOf(id);
  return tree === null ? [] : pathTo(tree, id).map((row) => row.id).filter((one) => one !== id);
}

/** What a thing of each kind lies in, where that is one kind only. */
const PARENT_KIND: Partial<Record<PlaceRef['kind'], PlaceRef['kind']>> = { thread: 'topic', topic: 'project', org: 'org' };

/** The place a node lies in: as the held tree knows it, else by the one kind such a node lies in. */
function parentPlace(place: PlaceRef): PlaceRef | null {
  if (place.parentId === null) return null;
  const tree = treeOf(place.parentId);
  const row = tree?.nodes.get(place.parentId);
  if (tree !== null && row !== undefined) return placeOfRow(row, row.kind === 'project' && tree.nodes.get(row.parent_id ?? '')?.kind === 'project');
  const kind = PARENT_KIND[place.kind];
  return kind === undefined ? null : placeById(place.parentId, kind);
}

export function usePlaceTrash(): (places: readonly PlaceRef[]) => Promise<TrashResult<PlaceRef, NodeDeleted>> {
  const { t } = useTranslation();
  const describe = useErrorText();
  const navigate = useNavigate();
  const viewing = useCurrentHeading()?.nodeId;
  return async (selected) => {
    const places = outermost(selected, (place) => place.id, (place) => above(place.id));
    const running = new Map(places.map((place) => [place.id, runningIn([place], (id) => {
      const tree = treeOf(id);
      return tree === null ? null : subtreeCounts(tree.nodes, id);
    }, (id) => {
      const state = treeOf(id)?.nodes.get(id)?.thread?.state;
      return state === 'running' || state === 'starting';
    })]));
    // Known before the request: once it is answered the tree has dropped what was deleted.
    const holding = viewing === undefined ? undefined : places.find((place) => place.id === viewing || above(viewing).includes(place.id));
    const leaveTo = holding === undefined ? null : parentPlace(holding);
    const trashOf = places.length === 0 ? null : parentPlace(places[0] as PlaceRef);
    const result = await trashPlaces(places);
    // A screen inside what went leaves for the place above it, or for the start when a whole org went.
    if (holding !== undefined && result.done.some(({ item }) => item.id === holding.id)) void navigate({ to: leaveTo === null ? '/' : placePath(leaveTo) });
    sayTrashed(t, {
      moved: result.done.map(({ item }) => item.name),
      refused: result.failed.map(({ item, error }) => ({ name: item.name, reason: describe(error).message })),
      stopped: result.done.reduce((total, { item }) => total + (running.get(item.id) ?? 0), 0),
      ...(trashOf === null ? {} : { show: () => openNodeDialog({ type: 'deleted', node: trashOf }) }),
      ...(result.done.some(({ receipt }) => receipt.deletion_id !== null) ? { undo: () => restorePlaces(result.done, (error) => describe(error).message) } : {}),
    });
    return result;
  };
}
