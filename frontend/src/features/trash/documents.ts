/** Documents and document folders to the trash of their place in one tree act, and the Undo of exactly that act.
 *
 * A selection is one deletion of at most fifty entries (the most one tree act takes); more are several acts. A run the
 * server refuses as a whole (one entry it may not delete) is applied entry by entry, so that the others still go and
 * the refused one is named. Each deletion is one group in the trash; Undo restores each group by the entries it was
 * given, which brings back exactly what that deletion took and leaves what was deleted before it in the trash.
 */
import { ApiError, ReceiptAcknowledged } from '@/api/client';
import { organizeDocuments } from '@/api/documentTree';
import type { DocumentTreeItem, DocumentTreeOperationBody } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';

import type { Refusal, UndoResult } from './tip';
import { chunks } from './trash';

/** The most entries one tree act takes, as the server bounds it. */
export const TREE_ACT_MAX = 50;

/** A document or folder as the person chose it, with the name the line uses. */
export interface TrashEntry {
  item: DocumentTreeItem;
  name: string;
}

export interface DocumentTrashResult {
  /** The deletions that were done, each by the entries it was given: the Undo of each names them again. */
  groups: TrashEntry[][];
  failed: { item: TrashEntry; error: unknown }[];
}

function act(command: 'delete' | 'restore', entries: readonly TrashEntry[]): Promise<unknown> {
  const body: DocumentTreeOperationBody = { command, items: entries.map((entry) => entry.item), to: null, name: null, version: null, deletion_plan: null, client_id: newActKey() };
  // The line follows the answer; the lists the act changed are read again after it.
  return organizeDocuments(body, { answerFirst: true }).catch((error: unknown) => {
    if (error instanceof ReceiptAcknowledged) return null;
    throw error;
  });
}

/** Whether the server answered with a refusal (so nothing of that act was done), not a failure to answer. */
function refused(error: unknown): boolean {
  return error instanceof ApiError && error.status >= 400 && error.status < 500;
}

/** Runs one command over the entries, each run as one act, and entry by entry where a run is refused as a whole. */
async function apply(command: 'delete' | 'restore', entries: readonly TrashEntry[]): Promise<DocumentTrashResult> {
  const result: DocumentTrashResult = { groups: [], failed: [] };
  for (const run of chunks(entries, TREE_ACT_MAX)) {
    try {
      await act(command, run);
      result.groups.push(run);
    } catch (error) {
      if (run.length === 1 || !refused(error)) {
        for (const item of run) result.failed.push({ item, error });
        continue;
      }
      for (const item of run) {
        try {
          await act(command, [item]);
          result.groups.push([item]);
        } catch (alone) {
          result.failed.push({ item, error: alone });
        }
      }
    }
  }
  return result;
}

export function trashDocuments(entries: readonly TrashEntry[]): Promise<DocumentTrashResult> {
  const seen = new Set<string>();
  return apply('delete', entries.filter((entry) => {
    const key = `${entry.item.kind}:${entry.item.id}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  }));
}

/** Brings back each group one act deleted; whatever cannot come back is named with why. */
export async function restoreDocuments(groups: readonly TrashEntry[][], reason: (error: unknown) => string): Promise<UndoResult> {
  const restored: string[] = [];
  const failed: Refusal[] = [];
  for (const group of groups) {
    const result = await apply('restore', group);
    restored.push(...result.groups.flat().map((entry) => entry.name));
    failed.push(...result.failed.map(({ item, error }) => ({ name: item.name, reason: reason(error) })));
  }
  return { restored, refused: failed };
}
