/** The same current reader identity fences both full and batched paragraph projections. */
import { client } from '@/api/client';
import type { SummaryChatRange } from '@/generated/api-types';
export function summaryReaderKey(): string {
  const state = client.session.getState();
  return JSON.stringify([state.userId, state.readerVersion ?? 0, state.status]);
}

export const resourceSummaryKey = (target: { kind: string; id: string }, range?: SummaryChatRange) => ['resource-summary', summaryReaderKey(), target.kind, target.id, range?.after ?? null, range?.through ?? null] as const;
