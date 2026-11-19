/** The person's recent and waiting threads in one org, joined only to its current readable tree. */
import type { TFunction } from 'i18next';

import { descendantsOf } from '@/features/navigation/places';
import type { NameHit, TreeNode, ThreadDisplayState } from '@/generated/api-types';
import { threadDisplayState } from '@/ui/ThreadState';
import { pathTo, type OrgTree } from '@/realtime/stores/tree';

export interface WorkThread {
  node: TreeNode;
  /** Names of the visible places between this org and the thread; never slugs or the thread again. */
  where: string;
}

export interface OrgWorkRows {
  waiting: WorkThread[];
  recent: WorkThread[];
}

function isWaiting(node: TreeNode): boolean {
  if (node.thread === null) return false;
  const state = threadDisplayState(node.thread.display);
  return state === 'waiting_approval' || state === 'blocked' || state === 'failed' || state === 'held';
}

/** A pending request uses the waiting glyph even while the rest of the agent keeps working. */
export function workDisplayState(node: TreeNode): ThreadDisplayState | null {
  return node.thread === null ? null : threadDisplayState(node.thread.display);
}

/** Visits provide order and personal identity, never permission or stale names/state after a revoke. */
export function orgWork(tree: OrgTree, orgId: string, visits: readonly NameHit[], localVisits: readonly string[] = []): OrgWorkRows {
  const root = tree.nodes.get(orgId);
  if (root?.deleted === true || root?.archived === true) return { waiting: [], recent: [] };
  const wholeTree = root?.kind === 'org' && root.parent_id === null;
  const below = new Set([...descendantsOf(tree.nodes, orgId)].map((node) => node.id));
  const visitedWithin = new Set(visits.filter((hit) => hit.kind === 'thread' && hit.path.some((place) => place.id === orgId)).map((hit) => hit.id));
  const threads = new Map<string, TreeNode>();
  for (const node of tree.nodes.values()) {
    // A direct grant may expose a thread without its private parent. The root owns the whole held
    // tree; within a department only its readable branch or an authorized visit's path proves scope.
    if ((wholeTree || below.has(node.id) || visitedWithin.has(node.id)) && node.kind === 'thread' && node.thread !== null && !node.deleted && !node.archived) threads.set(node.id, node);
  }
  const recentIds = [...new Set([...localVisits, ...visits.filter((hit) => hit.kind === 'thread' && hit.recent && !hit.archived).map((hit) => hit.id)])]
    .filter((id) => threads.has(id));
  const recentSet = new Set(recentIds);
  const row = (node: TreeNode): WorkThread => {
    const ancestors = pathTo(tree, node.id).slice(0, -1);
    const within = ancestors.slice(ancestors.findIndex((entry) => entry.id === orgId) + 1);
    return { node, where: within.map((entry) => entry.name).join(' / ') };
  };
  const waiting = [...threads.values()]
    .filter((node) => isWaiting(node) && (node.participating === true || node.thread?.waiting?.for_caller === true || recentSet.has(node.id)))
    .sort((one, other) => Number(other.thread?.waiting?.for_caller === true) - Number(one.thread?.waiting?.for_caller === true)
      || (other.thread?.last_activity_at ?? '').localeCompare(one.thread?.last_activity_at ?? '') || one.id.localeCompare(other.id))
    .map(row);
  const waitingIds = new Set(waiting.map(({ node }) => node.id));
  const recent = recentIds.flatMap((id) => {
    const node = threads.get(id);
    return node === undefined || waitingIds.has(id) ? [] : [row(node)];
  });
  return { waiting, recent };
}

/** One reason, in words. A viewer never inherits another person's “your approval” wording. */
export function workReason(node: TreeNode, t: TFunction): string {
  const thread = node.thread;
  if (thread === null) return '';
  const waiting = thread.waiting;
  if (waiting !== null && waiting !== undefined) {
    if (waiting.kind === 'answer') return t(waiting.for_caller ? 'places.org.work.yourAnswer' : 'places.org.work.answer');
    return t(waiting.for_caller ? 'places.org.work.yourApproval' : 'places.org.work.approval');
  }
  if (threadDisplayState(thread.display) === 'blocked') {
    if (thread.blocked_cause !== null && thread.blocked_cause !== undefined) return t(`places.org.work.blockedCause.${thread.blocked_cause}`);
    switch (thread.blocked_reason) {
      case 'unbound': return t('places.org.work.blocked.unbound');
      case 'agent_offline': return t('places.org.work.blocked.agent_offline');
      case 'agent_disabled': return t('places.org.work.blocked.agent_disabled');
      case 'provider_unavailable': return t('places.org.work.blocked.provider_unavailable');
      case 'provider_exhausted': return t('places.org.work.blocked.provider_exhausted');
      case 'environment_unavailable': return t('places.org.work.blocked.environment_unavailable');
      case 'binding_invalid': return t('places.org.work.blocked.binding_invalid');
      case 'provider_not_allowed': return t('places.org.work.blocked.provider_not_allowed');
      case 'model_not_set': return t('places.org.work.blocked.model_not_set');
      case 'model_not_allowed': return t('places.org.work.blocked.model_not_allowed');
      case 'quota_exceeded': return t('places.org.work.blocked.quota_exceeded');
      case 'attachments': return t('places.org.work.blocked.attachments');
      case 'workspace_trust': return t('places.org.work.blocked.workspace_trust');
      case 'switching': return t('places.org.work.blocked.switching');
      default: return t('places.org.work.blocked.error');
    }
  }
  if (threadDisplayState(thread.display) === 'waiting_approval') return t(thread.display.waiting_inputs > 0 ? 'day100.views.needsAttention' : 'places.org.work.approval');
  if (threadDisplayState(thread.display) === 'running') return t(thread.state === 'starting' ? 'places.org.work.starting' : 'places.org.work.running');
  return t(`state.${threadDisplayState(thread.display)}`);
}
