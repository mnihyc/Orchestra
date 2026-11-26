/** The place whose trash Show opens, from what a deleted thing says of where it lay. */
import { type PlaceRef, placeOfRow } from '@/features/navigation/places';
import type { NodeKind } from '@/generated/api-types';
import { useTreeStore } from '@/realtime/stores/tree';

/** The place `id`, as the held tree knows it, else by what the caller knows of it. */
export function placeById(id: string, kind: NodeKind = 'topic', name = ''): PlaceRef {
  for (const tree of useTreeStore.getState().byOrg.values()) {
    const row = tree.nodes.get(id);
    if (row !== undefined) return placeOfRow(row, row.kind === 'project' && tree.nodes.get(row.parent_id ?? '')?.kind === 'project');
  }
  return { id, kind, name, parentId: null, archived: false, status: null, folder: false };
}
