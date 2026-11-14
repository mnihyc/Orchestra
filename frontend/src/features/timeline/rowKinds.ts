/** What each item of a thread is to the person reading it: part of the conversation, the agent's work, or the program's own state.
 *
 * The conversation is what people and the agent say to each other, a question or an approval that waits for a person,
 * an error, and a word of the program that contradicts what the person chose. The work is what the agent did in a
 * turn: tool calls, file changes, plans, reasoning, sub-agents and compaction. Diagnostics are the harness's own state:
 * its session status, MCP server status, collaboration mode and instructions, what it merely echoes back after a
 * person chose it, notices that apply to future turns, events nobody has taught Orch to read and connection notes.
 * Diagnostics are kept as they were and drawn only where the reader asked for them; the other two kinds are drawn by
 * the depth of the reading.
 *
 * The kind is decided here, from the item alone, so that it can be tested by listing every kind of notice.
 */
import type { Item } from '@/generated/api-types';

export type RowKind = 'conversation' | 'work' | 'diagnostics';

type MarkerItem = Extract<Item, { kind: 'marker' }>;
type HarnessReport = Extract<MarkerItem['payload'], { marker: 'harness_changed' }>['data'];

const record = (value: unknown): Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

/** Program events that tell the reader something went wrong with the answer or the session, not merely that something happened. */
const EVENTS_THAT_MATTER = new Set(['model_refusal_fallback', 'model_refusal_no_fallback', 'session_writes_lost', 'mirror_error']);

/**
 * A report of the harness is diagnostics unless it contradicts what the person chose: a setting that differs from what
 * Orch asked for, an MCP server that failed to start, a hook that blocked or failed, a provider that refused or warned,
 * managed launch settings that differ, or an event that says the answer or the session was lost.
 */
export function harnessReportKind(report: HarnessReport): RowKind {
  if (report.diverged) return 'conversation';
  const value = record(report.value);
  switch (report.change) {
    case 'warning':
    case 'turn_diff_unavailable':
      return 'conversation';
    case 'context_compaction':
      return 'work';
    case 'mcp_startup':
      return value.status === 'failed' ? 'conversation' : 'diagnostics';
    case 'managed_settings_readback':
      return value.status === 'mismatch' ? 'conversation' : 'diagnostics';
    case 'rate_limit':
      return value.status === 'rejected' || value.status === 'allowed_warning' ? 'conversation' : 'diagnostics';
    case 'hook':
      return value.status === 'failed' || value.status === 'blocked' ? 'conversation' : 'diagnostics';
    case 'system_event':
      return typeof value.kind === 'string' && EVENTS_THAT_MATTER.has(value.kind) ? 'conversation' : 'diagnostics';
    default:
      // The session's status, the collaboration mode and its instructions, effort and model echoed back, approval and
      // sandbox modes, the environment's connection, reply buffering, native goals, session lifecycle, program
      // configuration and every event that is not recognised.
      return 'diagnostics';
  }
}

/** The kind of a line of the thread's history. */
export function markerKind(payload: MarkerItem['payload']): RowKind {
  switch (payload.marker) {
    case 'harness_changed':
      return harnessReportKind(payload.data);
    case 'compacted':
    case 'orch_context':
      return 'work';
    default:
      // Who created, renamed, switched, shared or moved the thread, and what happened to its environment, are what
      // people did and stay with the conversation.
      return 'conversation';
  }
}

/** The kind of an item. */
export function rowKind(item: Item): RowKind {
  switch (item.kind) {
    case 'user_message':
    case 'agent_message':
    case 'announcement':
    case 'error':
      return 'conversation';
    case 'approval':
      // A question that waits for a person is the conversation; one that has been decided is a record of work.
      return item.status === 'pending' ? 'conversation' : 'work';
    case 'reasoning':
    case 'plan':
    case 'tool_call':
      return 'work';
    case 'marker':
      return markerKind(item.payload);
  }
}

/** What a report and the notice error that goes with it are joined by: the turn it was made in and what it was about. */
export function reportKey(turnId: string | null, change: string): string {
  return `${turnId ?? ''}|${change}`;
}

/** The key of a contradicting report, or null when the report is not one that the conversation tells. */
export function toldReportKey(item: Item): string | null {
  if (item.kind !== 'marker' || item.payload.marker !== 'harness_changed') return null;
  if (harnessReportKind(item.payload.data) !== 'conversation') return null;
  return reportKey(item.payload.data.turn_id, item.payload.data.change);
}

/** The key of the notice error that a harness adds beside a report of its own; null for every other error. */
export function noticeKey(item: Item): string | null {
  if (item.kind !== 'error') return null;
  const details = record(item.payload.details);
  if (typeof details.change !== 'string') return null;
  return reportKey(typeof details.turn_id === 'string' ? details.turn_id : null, details.change);
}
