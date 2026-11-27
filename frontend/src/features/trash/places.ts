/** Places (projects, folders, topics, threads, orgs) to their parent's trash, and the Undo of exactly that act.
 *
 * Each place is deleted by its own request, one after another, without a plan read first: the server checks the
 * person's right and the place as it is now, settles its work and keeps it with everything in it until its retention
 * ends. The deletion group a request answers with is what its Undo names, so Undo brings back only what this act
 * deleted, as each thing stood (archived or not), and leaves anything deleted earlier in the trash. Each request goes
 * on at its answer: the line says what was done then, and the lists it changed are read again after it.
 */
import { ReceiptAcknowledged } from '@/api/client';
import { deleteNode, restoreNode } from '@/api/nodes';
import type { PlaceRef, SubtreeCounts } from '@/features/navigation/places';
import type { NodeDeleted } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';

import type { Refusal, UndoResult } from './tip';
import { eachInTurn, type TrashResult } from './trash';

/** Deletes each place in turn; a refusal is kept with its place and the rest go on. */
export function trashPlaces(places: readonly PlaceRef[]): Promise<TrashResult<PlaceRef, NodeDeleted>> {
  return eachInTurn(places, async (place) => {
    try {
      return await deleteNode(place.id, {}, newActKey(), { answerFirst: true });
    } catch (error) {
      // A bounded receipt is a done deletion whose group this reader is not told: it is moved, without an Undo here.
      if (error instanceof ReceiptAcknowledged) return { deletion_id: null };
      throw error;
    }
  });
}

/** Brings back what one act deleted, the last first; each that cannot come back is named with why. */
export async function restorePlaces(done: TrashResult<PlaceRef, NodeDeleted>['done'], reason: (error: unknown) => string): Promise<UndoResult> {
  const restored: string[] = [];
  const refused: Refusal[] = [];
  for (const { item, receipt } of [...done].reverse()) {
    if (receipt.deletion_id === null) continue;
    try {
      await restoreNode(item.id, receipt.deletion_id, newActKey(), { answerFirst: true });
      restored.push(item.name);
    } catch (error) {
      if (error instanceof ReceiptAcknowledged) restored.push(item.name);
      else refused.push({ name: item.name, reason: reason(error) });
    }
  }
  return { restored: restored.reverse(), refused };
}

/** The running threads a deletion of these places stops: those in them and the threads among them that run. */
export function runningIn(places: readonly PlaceRef[], countsOf: (id: string) => SubtreeCounts | null, runs: (id: string) => boolean): number {
  return places.reduce((total, place) => total + (countsOf(place.id)?.running ?? 0) + (place.kind === 'thread' && runs(place.id) ? 1 : 0), 0);
}
