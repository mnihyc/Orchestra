/** API client for Approval requests and decisions.
 *
 * One approval by its id, as the card in a conversation shows it, and the decision on it. A decision
 * that comes too late (somebody else answered first) is refused with the approval as it now stands,
 * which the card takes in place of its own.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { ApiError, api, ok } from '@/api/client';
import type { Approval, ApprovalDecision, ApprovalKind, ApprovalPage } from '@/generated/api-types';
import type { RequestDecisionStatus } from '@/generated/api-types';

export const approvalKey = (id: string) => ['approval', id] as const;

/** An approval by its id. */
export const approvalQuery = (id: string) =>
  queryOptions({
    queryKey: approvalKey(id),
    queryFn: ({ signal }): Promise<Approval> => ok(api.GET('/approvals/{id}', { params: { path: { id } }, signal })),
  });

/** Answers a request: an option the harness offered, the answers to its questions, or what an MCP server asked for. */
export function decideApproval(id: string, decision: ApprovalDecision): Promise<Approval> {
  return ok(api.POST('/approvals/{id}/decision', { params: { path: { id } }, body: decision }));
}

// ---------------------------------------------------------------- the lists

/** Every list of approvals is kept under this key, so that one call makes them all read again. */
export const approvalsKey = ['approvals'] as const;

/** How many approvals one page holds: a screenful of cards, each of which is tall. */
export const APPROVALS_PAGE = 20;

export interface ApprovalsFilter {
  /** `mine`: sent to the caller. `requested`: asked for by the caller. */
  scope: 'mine' | 'others' | 'requested';
  /** Waiting, or decided. */
  state: 'pending' | 'decided';
  /** With `mine`: everything the caller could decide, what went to people nearer the thing included. */
  all?: boolean;
  kind?: ApprovalKind;
}

/** One page of approvals, newest first. */
export function fetchApprovals(filter: ApprovalsFilter, cursor: string | null, signal?: AbortSignal): Promise<ApprovalPage> {
  const query: { scope: string; state: 'pending' | 'decided'; limit: number; all?: boolean; kind?: ApprovalKind; cursor?: string } = {
    scope: filter.scope,
    state: filter.state,
    limit: APPROVALS_PAGE,
  };
  if (filter.all === true) query.all = true;
  if (filter.kind !== undefined) query.kind = filter.kind;
  if (cursor !== null) query.cursor = cursor;
  return ok(api.GET('/approvals', { params: { query }, ...(signal === undefined ? {} : { signal }) }));
}

/** A list of approvals, page by page. */
export const approvalsQuery = (filter: ApprovalsFilter) =>
  infiniteQueryOptions({
    queryKey: [...approvalsKey, filter.scope, filter.state, filter.all === true, filter.kind ?? null] as const,
    queryFn: ({ pageParam, signal }): Promise<ApprovalPage> => fetchApprovals(filter, pageParam, signal),
    initialPageParam: null as string | null,
    getNextPageParam: (last: ApprovalPage) => last.next_cursor,
  });

/** Takes back a request for access that the caller made. */
export function cancelApproval(id: string): Promise<Approval> {
  return ok(api.POST('/approvals/{id}/cancel', { params: { path: { id } } }));
}

/** The approval as it stands, when a decision came too late: somebody answered first. */
export function answeredMeanwhile(error: unknown): Approval | null {
  if (!(error instanceof ApiError) || !error.is('approval.already_decided')) return null;
  const current = error.problem.current;
  return typeof current === 'object' && current !== null && 'id' in current ? (current as Approval) : null;
}

/** The same recorded status for a plan or model-use wait somebody else already answered. */
export function sourceDecisionOf(error:unknown):RequestDecisionStatus|null {
  if(!(error instanceof ApiError) || !error.is('approval.already_decided'))return null;
  const current=error.problem.current;
  if(typeof current!=='object'||current===null||!('action' in current)||!('decided_at' in current)||!('decided_by' in current))return null;
  return (current.action===null||typeof current.action==='string'&&['proceed','keep_planning','run'].includes(current.action)) && typeof current.decided_at==='string' ? current as RequestDecisionStatus : null;
}
