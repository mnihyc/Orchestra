/** The plan of an archive read for the line under the options: how much against what the place allows, by how much it is over, and what is left out and where.
 *
 * The server says what an archive would hold, what the place allows, and what stands in its way. This turns
 * that into the figures a person reads: the size and the number of things against their limits, the excess when
 * there is one, the hours it is kept, and how many things they may read and may not take away. A place that
 * allows no bytes at all is told apart from one that is only too small for what was asked.
 */
import type { ExportPlan } from '@/generated/api-types';

/** Several quick changes are one request for a fresh plan. */
export const PLAN_PAUSE_MS = 350;

export type PlanState =
  /** It can be asked for. */
  | 'ok'
  /** There is nothing in it that the caller may take away. */
  | 'nothing'
  /** The place allows no export at all. */
  | 'none-allowed'
  /** It passes what the place allows, by the figures below. */
  | 'too-large';

export interface PlanReading {
  state: PlanState;
  bytes: number;
  maxBytes: number;
  /** Conversations, chats and documents together: the number the place limits. */
  items: number;
  maxItems: number;
  keepHours: number;
  /** By how many bytes the archive passes what the place allows; 0 when it does not. */
  overBytes: number;
  /** By how many things; 0 when it does not. */
  overItems: number;
  /** How many things the caller may read and may not take away, in all and by the place they lie in. */
  notExportable: number;
  notExportableIn: ReadonlyMap<string, number>;
  /** How many files were left out for their size. */
  tooLarge: number;
}

/** Reads a plan. */
export function readPlan(plan: ExportPlan): PlanReading {
  const items = plan.counts.conversations + plan.counts.chats + plan.counts.documents;
  const overBytes = Math.max(0, plan.bytes - plan.limits.max_bytes);
  const overItems = Math.max(0, items - plan.limits.max_items);
  const notExportableIn = new Map<string, number>();
  let notExportable = 0;
  let tooLarge = 0;
  for (const left of plan.left_out) {
    if (left.what === 'too_large') {
      tooLarge += left.count;
      continue;
    }
    notExportable += left.count;
    if (left.where !== null) notExportableIn.set(left.where.id, (notExportableIn.get(left.where.id) ?? 0) + left.count);
  }
  const blocked = (code: string) => plan.blockers.some((blocker) => blocker.code === code);
  let state: PlanState = 'ok';
  if (plan.limits.max_bytes <= 0) state = 'none-allowed';
  else if (items === 0 || blocked('export.nothing')) state = 'nothing';
  else if (overBytes > 0 || overItems > 0 || blocked('export.too_large') || blocked('export.too_many')) state = 'too-large';
  return {
    state,
    bytes: plan.bytes,
    maxBytes: plan.limits.max_bytes,
    items,
    maxItems: plan.limits.max_items,
    keepHours: plan.limits.keep_hours,
    overBytes,
    overItems,
    notExportable,
    notExportableIn,
    tooLarge,
  };
}
