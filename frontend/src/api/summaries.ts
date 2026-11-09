/** Summary configuration uses the ordinary authenticated client and keeps its original reader. */
import { queryOptions } from '@tanstack/react-query';
import { accepted, api, ok } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import type { SummaryChatRange, SummaryOptionsReceipt, SummaryOptionsView, SummaryReceipt, SummarySnapshot, SummaryTarget, SummaryTargetOptions } from '@/generated/api-types';
import { summaryReaderKey } from '@/api/summaryReader';
export { summaryReaderKey } from '@/api/summaryReader';
export { searchSummaryProjectionQuery } from '@/api/summaryProjections';

// Keep the boundary check for malformed wire replies, including a snapshot in place of a receipt.
const receiptProjection = (value: unknown): boolean => value === 'receipt';

function sameReader(): () => void {
  const reader = summaryReaderKey();
  return () => { if (reader !== summaryReaderKey()) throw new DOMException('The summary reader changed', 'AbortError'); };
}
export const documentSummaryOptionsKey = (id: string) => ['document-summary-options', summaryReaderKey(), id] as const;

export const documentSummaryOptionsQuery = (id: string) => {
  const current = sameReader();
  return queryOptions({
    queryKey: documentSummaryOptionsKey(id),
    queryFn: async ({ signal }): Promise<SummaryOptionsView> => {
      current();
      const reply = await ok(api.GET('/documents/{id}/summary-options', { params: { path: { id } }, signal }));
      current();
      if (reply.target.kind !== 'document' || reply.target.id !== id) throw new NetworkError('malformed');
      return reply;
    },
    staleTime: 0,
    retry: false,
  });
};

/** The acknowledgement contains metadata only. Reading options afterwards is a separate act. */
export async function setDocumentSummaryOptions(id: string, options: SummaryTargetOptions | null, expectedRevision: number): Promise<SummaryOptionsReceipt> {
  const current = sameReader();
  current();
  const receipt = await accepted(api.PUT('/documents/{id}/summary-options', {
    params: { path: { id } }, body: { options, expected_revision: expectedRevision },
  }));
  current();
  if (!receiptProjection(receipt.projection) || receipt.target.kind !== 'document' || receipt.target.id !== id
    || !Number.isSafeInteger(receipt.revision) || receipt.revision < 0) throw new NetworkError('malformed');
  return receipt;
}

/** Ordinary source readers and person-issued receipts share the actual generated contract. */
export type ResourceSummaryTarget = SummaryTarget;
export type ResourceSummaryRange = SummaryChatRange;
export type ResourceSummaryReply = SummarySnapshot;
/** Browsers represent timer delays as signed32-bit milliseconds; longer native waits are followed in chunks. */
const MAX_TIMER_DELAY_MS = 2_147_483_647;
export const resourceSummaryKey = (target: { kind: string; id: string }, range?: ResourceSummaryRange) => ['resource-summary', summaryReaderKey(), target.kind, target.id, range?.after ?? null, range?.through ?? null] as const;
export const resourceSummaryQuery = (target: ResourceSummaryTarget, range?: ResourceSummaryRange) => {
  const current = sameReader();
  return queryOptions({
    queryKey: resourceSummaryKey(target, range),
    queryFn: async ({ signal }): Promise<ResourceSummaryReply> => {
      current();
      const reply = await ok(api.GET('/summaries/{kind}/{id}', {
        params: { path: { kind: target.kind, id: target.id }, query: { after: range?.after ?? null, through: range?.through ?? null } }, signal,
      }));
      current();
      if (reply.target.kind !== target.kind || reply.target.id !== target.id || reply.root_org_id === '') throw new NetworkError('malformed');
      return reply;
    },
    staleTime: 0, retry: false,
    // Only the server's pending-work pace follows free reads; it never submits/retries paid work.
    refetchInterval: (query) => {
      if (query.state.status === 'error' || query.state.fetchStatus !== 'idle') return false;
      const after = query.state.data?.poll_after_ms;
      if (after === null || after === undefined || !Number.isSafeInteger(after) || after <= 0) return false;
      const elapsed = query.state.data?.state === 'scheduled' ? Math.max(0, Date.now() - query.state.dataUpdatedAt) : 0;
      return Math.min(Math.max(1, after - elapsed), MAX_TIMER_DELAY_MS);
    },
    refetchOnWindowFocus: (query) => {
      const after = query.state.data?.poll_after_ms;
      return query.state.status !== 'error' && after !== null && after !== undefined && Date.now() - query.state.dataUpdatedAt >= after;
    },
  });
};
export async function refreshResourceSummary(target: ResourceSummaryTarget, clientId: string, range?: ResourceSummaryRange): Promise<SummaryReceipt> {
  const current = sameReader(); current();
  const receipt = await accepted(api.POST('/summaries/{kind}/{id}/refresh', {
    params: { path: { kind: target.kind, id: target.id } }, body: { client_id: clientId, ...(range === undefined ? {} : { range }) },
  }));
  current();
  if (!receiptProjection(receipt.projection) || receipt.target.kind !== target.kind || receipt.target.id !== target.id) throw new NetworkError('malformed');
  return receipt;
}
