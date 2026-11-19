/** Live request snapshots and confirmed decisions settle a timeline card without another read. */
import type { Approval, Item } from '@/generated/api-types';
import { approvalSnapshot } from '@/features/approvals/snapshot';

type ApprovalItem = Extract<Item, { kind: 'approval' }>;

/** A terminal item status is the approval's committed state, even before a legacy lookup catches up. */
function ended(status: Item['status']): Approval['state'] | null {
  switch (status) {
    case 'approved': return 'approved';
    case 'answered': return 'answered';
    case 'denied': return 'denied';
    case 'cancelled': return 'cancelled';
    case 'expired': return 'expired';
    default: return null;
  }
}

/** A fresh live outcome wins; a local confirmed outcome cannot be reopened by an old pending item. */
export function approvalAtItem(item: ApprovalItem, held: unknown): Approval | undefined {
  const inline = approvalSnapshot(item.payload.approval, item.payload.approval_id);
  const cached = approvalSnapshot(held, item.payload.approval_id);
  const current = inline !== undefined && inline.state !== 'pending' ? inline
    : cached !== undefined && cached.state !== 'pending' ? cached : inline ?? cached;
  const state = ended(item.status);
  return current?.state === 'pending' && state !== null ? { ...current, state, can_decide: false } : current;
}


/** Older records stored human content as approved; its meaning is still an answer, not an action grant. */
export function approvalOutcome(approval: Approval): Approval['state'] {
  return approval.state === 'approved' && approval.kind === 'harness'
    && (approval.request.request_kind === 'user_input' || approval.request.request_kind === 'mcp_elicitation')
    ? 'answered' : approval.state;
}
