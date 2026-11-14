/** What each item of a thread is to the person reading it: part of the conversation, the agent's work, or the program's own state.
 *
 * The conversation is what people and the agent say to each other, a question or an approval that waits for a person,
 * an error, a retry notice, the compaction of the conversation, a model that was changed, and a word of the program that
 * contradicts what the person chose; it stands where it was made. The work is what the agent did in a turn: tool calls,
 * file changes, plans, reasoning, sub-agents and the approvals that have been decided; in Normal reading it is drawn as the
 * one group of its turn. Diagnostics are the harness's own state: its session status, MCP server status, collaboration
 * mode and instructions, what it merely echoes back after a person chose it, notices that apply to future turns, events
 * nobody has taught Orch to read and connection notes. Diagnostics are kept as they were and drawn only in Diagnostics
 * reading.
 *
 * The kind is decided here, from the item alone, so that it can be tested by listing every kind of notice.
 */
import type { Item } from '@/generated/api-types';
import type { WorkDetail } from './rows';

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
    case 'context_compaction':
      return 'conversation';
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

/** An actionable report, compaction or model change stays in the conversation when ordinary lifecycle history is hidden. */
export function inlineMarker(item: Item): boolean {
  return item.kind === 'marker' && (item.payload.marker === 'compacted' || item.payload.marker === 'model_changed'
    || item.payload.marker === 'harness_changed' && harnessReportKind(item.payload.data) === 'conversation');
}

/** Reports and their paired notices share a turn, change and named server or setting. */
export function reportKey(turnId: string | null, change: string, subject: string | null = null): string {
  return JSON.stringify([turnId, change, subject]);
}

/** The key of a contradicting report, or null when the report is not one that the conversation tells. */
export function toldReportKey(item: Item): string | null {
  if (item.kind !== 'marker' || item.payload.marker !== 'harness_changed') return null;
  if (harnessReportKind(item.payload.data) !== 'conversation') return null;
  const data = item.payload.data;
  const scoped = data.change === 'mcp_startup' || data.change === 'managed_setting';
  if (scoped && (typeof data.subject !== 'string' || data.subject.length === 0)) return null;
  return reportKey(data.turn_id, data.change, scoped ? data.subject ?? null : null);
}

/** The key of the notice error that a harness adds beside a report of its own; null for every other error. */
export function noticeKey(item: Item): string | null {
  if (item.kind !== 'error') return null;
  if (item.payload.code !== 'harness_diverged' && item.payload.code !== 'mcp_server_failed') return null;
  const details = record(item.payload.details);
  if (typeof details.change !== 'string') return null;
  let subject: string | null = null;
  if (details.change === 'mcp_startup' || details.change === 'managed_setting') {
    const named = details.change === 'mcp_startup' ? details.server : details.setting;
    if (typeof named !== 'string' || named.length === 0) return null;
    subject = named;
  }
  return reportKey(typeof details.turn_id === 'string' ? details.turn_id : null, details.change, subject);
}

interface Reading {
  detail?: WorkDetail;
  hideHistory?: boolean;
  revealed?: ReadonlySet<string>;
}

/** Lifecycle history follows its switch; work, actionable reports and aside destinations keep their own rules. */
function ordinaryHistory(item: Item): boolean {
  return item.kind === 'marker' && !inlineMarker(item) && !['harness_changed', 'orch_context', 'background_wake', 'side_started', 'side_closed'].includes(item.payload.marker);
}

/** Every conversation surface uses the same visibility, report pairing and completed-answer rule. */
export function hiddenConversationItems(order: readonly string[], items: ReadonlyMap<string, Item>, reading: Reading): ReadonlySet<string> {
  const hidden = new Set<string>();
  if (reading.detail !== 'diagnostics') {
    const told = new Set<string>();
    const inStream = (item: Item, key: string): string => JSON.stringify([item.thread_id, item.stream, key]);
    for (const id of order) {
      const item = items.get(id);
      const key = item === undefined ? null : toldReportKey(item);
      if (item !== undefined && key !== null) told.add(inStream(item, key));
    }
    for (const id of order) {
      const item = items.get(id);
      if (item === undefined) continue;
      const key = noticeKey(item);
      if (rowKind(item) === 'diagnostics' || key !== null && told.has(inStream(item, key))) hidden.add(id);
    }
    // Only a completed final can replace completed commentary. A retry's equal commentary and
    // a growing partial answer retain their chronology; Diagnostics keeps every native message.
    const finals = new Set<string>();
    for (let at = order.length - 1; at >= 0; at--) {
      const id = order[at] ?? '';
      const item = items.get(id);
      if (item?.kind !== 'agent_message' || item.turn_id === null || item.status !== 'completed' || item.payload.text.trim().length === 0) continue;
      const key = JSON.stringify([item.thread_id, item.turn_id, item.stream, item.payload.text]);
      if (item.payload.phase === 'commentary' && finals.has(key)) hidden.add(id);
      if (item.payload.phase === 'final') finals.add(key);
    }
  }
  if (reading.hideHistory === true) for (const id of order) {
    const item = items.get(id);
    if (item !== undefined && ordinaryHistory(item)) hidden.add(id);
  }
  for (const id of reading.revealed ?? []) hidden.delete(id);
  return hidden;
}
