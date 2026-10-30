/** Per-row guarded writes, uncertain answers and exact successful inverses; there is no batch write endpoint. */
import { ApiError } from '@/api/client';
import { applyArchivePlan, moveNode, previewMove, deleteNode, readNode, restoreNode } from '@/api/nodes';
import type { Node, NodeArchivePlan, NodeDeletePlan, NodeMovePlan } from '@/generated/api-types';

export type ReviewAction = 'archive' | 'unarchive' | 'move' | 'delete';
export type PlacePlan =
  | { kind: 'archive' | 'unarchive'; before: Node; plan: NodeArchivePlan }
  | { kind: 'move'; before: Node; plan: NodeMovePlan }
  | { kind: 'delete'; before: Node; plan: NodeDeletePlan };
export type Receipt =
  | { kind: 'archive'; node: Node; undo: NodeArchivePlan | null }
  | { kind: 'move'; node: Node; from: string }
  | { kind: 'delete'; deletion: string | null };
export interface RowResult {
  review: PlacePlan;
  state: 'queued' | 'applying' | 'succeeded' | 'refused' | 'uncertain' | 'skipped' | 'observed';
  receipt?: Receipt;
  error?: unknown;
  observed?: Node;
  undone?: boolean;
}
export function blockersOf(review: PlacePlan): number { return review.kind === 'delete' ? 0 : review.plan.blockers.length; }
export async function applyPlace(review: PlacePlan, key: string): Promise<Receipt> {
  switch (review.kind) {
    case 'archive': case 'unarchive': {
      const result = await applyArchivePlan(review.plan, key);
      return { kind: 'archive', node: result.node, undo: result.undo };
    }
    case 'move': {
      if (review.before.kind === 'thread') throw new Error('A thread uses its own move review.');
      return { kind: 'move', node: await moveNode(review.before.kind, review.before.id, review.plan.to.id, review.plan.revision, review.plan, key), from: review.plan.from.id };
    }
    case 'delete': {
      const result = await deleteNode(review.before.id, { preflight_id: review.plan.preflight_id, expected_revision: review.plan.revision }, key);
      return { kind: 'delete', deletion: result.deletion_id };
    }
  }
}
/** One refusal never pretends that the remaining rows were applied. */
export async function applyRows(plans: readonly PlacePlan[], keys: ReadonlyMap<string, string>, update: (result: RowResult) => void): Promise<RowResult[]> {
  const results: RowResult[] = [];
  for (const review of plans) {
    let result: RowResult;
    if (blockersOf(review) > 0) result = { review, state: 'skipped' };
    else {
      update({ review, state: 'applying' });
      try { result = { review, state: 'succeeded', receipt: await applyPlace(review, keys.get(review.before.id) ?? '') }; }
      catch (error) { result = { review, state: error instanceof ApiError && error.status < 500 ? 'refused' : 'uncertain', error }; }
    }
    results.push(result); update(result);
  }
  return results;
}
export function canUndo(result: RowResult): boolean {
  if (result.state !== 'succeeded' || result.receipt === undefined || result.undone === true) return false;
  return result.receipt.kind === 'archive' ? result.receipt.undo !== null : result.receipt.kind === 'delete' ? result.receipt.deletion !== null : true;
}
/** A read of a desired state is not a write receipt. */
export async function undoPlace(result: RowResult, changed: string): Promise<void> {
  const receipt = result.receipt;
  if (!canUndo(result) || receipt === undefined) return;
  if (receipt.kind === 'archive') { if (receipt.undo !== null) await applyArchivePlan(receipt.undo); return; }
  if (receipt.kind === 'delete') { if (receipt.deletion !== null) await restoreNode(result.review.before.id, receipt.deletion); return; }
  const current = await readNode(result.review.before.id);
  if (current.revision !== receipt.node.revision || current.kind === 'thread') throw new Error(changed);
  const plan = await previewMove(current.kind, current.id, receipt.from);
  if (plan.revision !== receipt.node.revision) throw new Error(changed);
  if (plan.blockers.length > 0) throw new Error(plan.blockers.map((blocker) => blocker.message).join(' '));
  await moveNode(current.kind, current.id, receipt.from, receipt.node.revision, plan);
}
