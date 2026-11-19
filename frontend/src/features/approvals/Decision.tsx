/** Race-aware confirmed outcome.
 *
 * Answering what the agent asks, and what that comes to when somebody else answers first. A decision
 * is shown as pending, under the person's name, until the server has answered; nothing is assumed
 * before. If somebody else decided in the meantime the server says so and sends the approval as it
 * now stands, which the card takes in place of its own and says "Already answered by <name>".
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';

import { client } from '@/api/client';
import { planningAfter } from '@/features/approvals/PlanAnswer';
import { answeredMeanwhile, approvalKey, decideApproval } from '@/api/approvals';
import type { Approval, ApprovalDecision } from '@/generated/api-types';

export interface DecisionState {
  /** Sends a decision. */
  decide: (decision: ApprovalDecision, label?: string) => void;
  pending: boolean;
  /** The label of the option that was pressed, while its answer is awaited. */
  pressed: string | null;
  error: unknown;
  /** Who got there first, when the decision came too late. */
  lost: Approval | null;
}

type Reader = Pick<ReturnType<typeof client.session.getState>, 'userId' | 'readerVersion' | 'status'>;
interface Call { approvalId: string; decision: ApprovalDecision; reader: Reader }
function ownsReader(call: Call): boolean {
  const now = client.session.getState();
  return call.reader.userId === now.userId && call.reader.readerVersion === now.readerVersion && call.reader.status === now.status;
}

export function useDecision(approvalId: string, handlers: { onDecided?: (approval: Approval) => void; onLost?: (approval: Approval) => void } = {}): DecisionState {
  const queryClient = useQueryClient();
  const [feedback, setFeedback] = useState<{ call: Call; pressed: string | null; lost: Approval | null } | null>(null);
  const current = (call: Call) => call.approvalId === approvalId && ownsReader(call);
  const mutation = useMutation({
    mutationFn: (call: Call) => decideApproval(call.approvalId, call.decision),
    onSuccess: (approval, call) => {
      if (!ownsReader(call)) return;
      queryClient.setQueryData(approvalKey(call.approvalId), approval);
      if (!current(call)) return;
      if (approval.kind === 'harness' && approval.request.request_kind === 'plan'
        && (approval.state === 'approved' || approval.state === 'denied')) {
        planningAfter(approval, approval.state === 'denied');
      }
      handlers.onDecided?.(approval);
    },
    onError: (error, call) => {
      if (!ownsReader(call)) return;
      const decided = answeredMeanwhile(error);
      if (decided?.id !== call.approvalId) return;
      queryClient.setQueryData(approvalKey(call.approvalId), decided);
      if (!current(call)) return;
      setFeedback((held) => held?.call === call ? { ...held, lost: decided } : held);
      handlers.onLost?.(decided);
    },
    onSettled: (_data, _error, call) => {
      setFeedback((held) => held?.call === call ? { ...held, pressed: null } : held);
    },
  });
  const shown = feedback !== null && current(feedback.call) ? feedback : null;
  const active = mutation.variables !== undefined && current(mutation.variables);
  const lost = shown?.lost ?? null;
  return {
    decide: (decision, label) => {
      const { userId, readerVersion, status } = client.session.getState();
      const call = { approvalId, decision, reader: { userId, readerVersion, status } };
      setFeedback({ call, pressed: label ?? null, lost: null });
      mutation.mutate(call);
    },
    pending: active && mutation.isPending,
    pressed: shown?.pressed ?? null,
    error: active && lost === null ? mutation.error : null,
    lost,
  };
}
