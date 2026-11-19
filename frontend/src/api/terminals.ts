/** API client for Shells that are their opener's alone, the ticket that takes one to another screen, the choice of which to close at a limit, and diagnostic sockets.
 *
 * A person's shells in an environment: the list of the ones they have there, whichever thread each
 * was opened from; opening one (with, at a limit, the ones they give up in the same step); a ticket
 * for attaching to one again, from this browser or another; closing one; and what a closed shell last
 * printed. The shell itself is a socket of its own (`realtime/terminal.ts`).
 */
import { queryOptions } from '@tanstack/react-query';

import { ApiError, api, done, ok } from '@/api/client';
import { liveValue } from '@/api/liveValue';
import type { EnvironmentTerminalCreateBody, EnvironmentTerminalCreateReply, TerminalSession, TerminalTicket } from '@/generated/api-types';
import type { Door } from '@/api/workspace';

/** The shells the list is asked for at once: a person has a handful, and the limit is a setting of the place. */
export const SHELLS_LISTED = 50;

const through = (door: Door): { thread?: string } => (door.threadId === null ? {} : { thread: door.threadId });

export const terminalKeys = {
  list: (door: Door) => ['terminals', door.environmentId] as const,
};

/** The caller's own shells in the environment that are not closed. */
export const shellsQuery = (door: Door) =>
  queryOptions({
    queryKey: terminalKeys.list(door),
    queryFn: ({ signal }): Promise<TerminalSession[]> => liveValue(terminalKeys.list(door), signal),
  });

/** The shells the caller closed or lost, most recent first: what the panel offers to show the last of. */
export const closedShellsQuery = (door: Door) =>
  queryOptions({
    queryKey: [...terminalKeys.list(door), 'closed'] as const,
    queryFn: ({ signal }): Promise<TerminalSession[]> => liveValue([...terminalKeys.list(door), 'closed'], signal),
  });

/** Opens a shell, closing first the ones named, which is how one more than the limit is made. */
export function openShell(door: Door, body: EnvironmentTerminalCreateBody): Promise<EnvironmentTerminalCreateReply> {
  return ok(api.POST('/environments/{id}/terminals', { params: { path: { id: door.environmentId }, query: through(door) }, body }));
}

/** A ticket for attaching to a shell again; attaching ends an attachment elsewhere. */
export function shellTicket(terminalId: string): Promise<TerminalTicket> {
  return ok(api.POST('/terminals/{terminal_id}/ticket', { params: { path: { terminal_id: terminalId } } }));
}

/** Closes a shell: its person's act, or an environment manager's. */
export function closeShell(terminalId: string): Promise<TerminalSession> {
  return ok(api.DELETE('/terminals/{terminal_id}', { params: { path: { terminal_id: terminalId } } }));
}

/** What a shell last printed, as it was kept when it closed; an empty text where nothing was kept. */
export async function shellRecording(terminalId: string): Promise<Uint8Array | null> {
  const answer = await api.GET('/terminals/{terminal_id}/recording', {
    params: { path: { terminal_id: terminalId } },
    parseAs: 'arrayBuffer',
  });
  if (answer.response.status === 404) return null;
  if (!answer.response.ok) await done(Promise.resolve(answer));
  return new Uint8Array(answer.data as unknown as ArrayBuffer);
}

/** The reasons for which opening a shell is refused though the caller holds the permission. */
export type NotAllowedReason = 'disabled_by_policy' | 'environment_switch_off' | 'machine_policy' | 'environment_offline';

const REASONS: readonly string[] = ['disabled_by_policy', 'environment_switch_off', 'machine_policy', 'environment_offline'];

/** Why a shell was not opened, when the refusal is one that no permission lifts. */
export function notAllowedReason(error: unknown): NotAllowedReason | null {
  if (!(error instanceof ApiError) || !error.is('terminal.not_allowed')) return null;
  const reason = (error.problem as unknown as Record<string, unknown>).reason;
  return typeof reason === 'string' && REASONS.includes(reason) ? (reason as NotAllowedReason) : null;
}

/** What a refusal at a limit says: the caller's own shells there, oldest use first, and which limit it was. */
export interface AtLimit {
  mine: TerminalSession[];
  limit: 'person' | 'environment';
  /** How many shells are open there in all, and how many people have them, where the server says so. */
  open: number | null;
  people: number | null;
}

/** Reads the refusal of a shell that would be one too many. */
export function atLimit(error: unknown): AtLimit | null {
  if (!(error instanceof ApiError) || !error.is('conflict.exists')) return null;
  const problem = error.problem as unknown as Record<string, unknown>;
  if (problem.limit !== 'person' && problem.limit !== 'environment') return null;
  const mine = Array.isArray(problem.mine) ? (problem.mine as TerminalSession[]) : [];
  return {
    mine,
    limit: problem.limit,
    open: typeof problem.open === 'number' ? problem.open : Array.isArray(problem.open) ? problem.open.length : null,
    people: typeof problem.people === 'number' ? problem.people : null,
  };
}
