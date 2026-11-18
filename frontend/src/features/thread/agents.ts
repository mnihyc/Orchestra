/** The sub-agents of a thread, running and finished, read from the steps that started them.
 *
 * Each is one entry: its name, where it stands, how long it has run, what it used where that was recorded, and one line of what it is
 * doing or what it came to. A finished one stays in the list. The current native roster takes precedence over recorded
 * transcript state, including a child that outlives its parent's turn or was lost with its session.
 */
import type { TFunction } from 'i18next';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { outlineQuery } from '@/api/threads';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useDebounced } from '@/lib/debounce';
import { useTranslation } from 'react-i18next';

import { itemText } from '@/features/timeline/ItemActions';
import { recordedItem } from '@/features/timeline/historical';
import { isOver } from '@/features/timeline/rows';
import { describeStep } from '@/features/timeline/titles';
import type { BackgroundWork, OutlineEntry, SubagentState } from '@/generated/api-types';
import { type ThreadView, useThreadPart } from '@/realtime/stores/thread';

export interface AgentEntry {
  /** The step that started it. */
  id: string;
  /** Where its own items are. */
  stream: string;
  turnId: string | null;
  name: string;
  /** `recorded`: a session that has ended marked it as running; nothing says how it came out. */
  state: SubagentState | 'lost' | 'recorded';
  startedAt: string | null;
  durationMs: number | null;
  /** What it wrote, where its program recorded it. */
  tokensOut: number | null;
  /** A question or an approval of its own waits for a person. */
  waiting: boolean;
  line: { kind: 'activity' | 'result'; text: string } | null;
  /** The running work the roster knows, for Stop. */
  task: BackgroundWork | null;
}

export type AgentGroup = 'active' | 'done';

export function agentGroup(entry: AgentEntry): AgentGroup {
  return entry.state === 'running' ? 'active' : 'done';
}

const firstLine = (text: string): string => text.trim().split(/\r?\n/)[0] ?? '';

/** What it is doing now (while it runs) or what it came to (when it is over), in one line of its own items. */
function lineOf(view: Pick<ThreadView, 'items' | 'streams'>, stream: string, running: boolean, t: TFunction): AgentEntry['line'] {
  const ids = view.streams.get(stream) ?? [];
  for (let at = ids.length - 1; at >= 0; at -= 1) {
    const item = view.items.get(ids[at] ?? '');
    if (item === undefined) continue;
    if (!running) {
      if (item.kind === 'agent_message' && item.payload.text.trim() !== '') return { kind: 'result', text: firstLine(item.payload.text) };
      continue;
    }
    if (item.kind === 'tool_call') return { kind: 'activity', text: describeStep(item, t).text };
    if (item.kind === 'reasoning') return { kind: 'activity', text: t('thread.now.thinking') };
    if (item.kind === 'approval' && item.status === 'pending') return { kind: 'activity', text: t('thread.now.approval') };
    if (item.kind === 'agent_message' && item.payload.text.trim() !== '') return { kind: 'activity', text: firstLine(itemText(item)) };
  }
  return null;
}

export function listAgents(view: Pick<ThreadView, 'items' | 'streams' | 'turns' | 'segments'>, tasks: readonly BackgroundWork[], t: TFunction, stored: readonly OutlineEntry[] = []): AgentEntry[] {
  const entries = new Map<string, { seq: number; updated: number; entry: AgentEntry }>();
  for (const point of stored) {
    const facts = point.agent;
    if (point.kind !== 'subagent' || facts === undefined || facts === null) continue;
    const state = facts.state === 'running' && facts.recorded ? 'recorded' : facts.state;
    entries.set(point.item_id, { seq: point.seq, updated: facts.updated_seq, entry: {
      id: point.item_id, stream: facts.stream, turnId: facts.turn_id, name: facts.name === null || facts.name === '' ? t('timeline.step.subagentUnnamed') : facts.name, state,
      startedAt: facts.started_at, durationMs: facts.duration_ms, tokensOut: facts.tokens_out,
      waiting: false, line: facts.result === null ? null : { kind: state === 'running' ? 'activity' : 'result', text: facts.result }, task: null,
    } });
  }
  for (const item of view.items.values()) {
    if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'subagent') continue;
    if ((entries.get(item.id)?.updated ?? -1) > item.seq_updated) continue;
    const tool = item.payload.tool;
    const turn = item.turn_id === null ? undefined : view.turns.get(item.turn_id);
    const recorded = tool.state === 'running' && (recordedItem(item, view.segments) || turn !== undefined && isOver(turn));
    const state = recorded ? 'recorded' : tool.state;
    const output = turn?.usage_detail?.by_subagent.find((share) => share.stream === tool.child_stream)?.tokens.output_tokens;
    entries.set(item.id, { seq: item.seq_created, updated: item.seq_updated, entry: {
      id: item.id, stream: tool.child_stream, turnId: item.turn_id, name: tool.name === null || tool.name === '' ? t('timeline.step.subagentUnnamed') : tool.name, state,
      startedAt: item.payload.started_at, durationMs: item.payload.duration_ms, tokensOut: output ?? null, waiting: false,
      line: lineOf(view, tool.child_stream, state === 'running', t), task: null,
    } });
  }
  // A native roster is independently authoritative even when its start row is outside the item window.
  for (const task of tasks) {
    if (task.kind !== 'subagent') continue;
    const held = entries.get(task.item_id);
    if (held !== undefined && held.entry.turnId !== null && held.entry.turnId !== task.turn_id) continue;
    const state = task.state === 'stopped' ? 'interrupted' : task.state;
    const entry: AgentEntry = held === undefined ? {
      id: task.item_id, stream: '', turnId: task.turn_id, name: task.label || t('timeline.step.subagentUnnamed'), state,
      startedAt: null, durationMs: null, tokensOut: null, waiting: false, line: null, task: null,
    } : { ...held.entry, state };
    entry.task = state === 'running' ? task : null;
    entries.set(task.item_id, { seq: held?.seq ?? Number.MAX_SAFE_INTEGER, updated: held?.updated ?? 0, entry });
  }
  for (const { entry } of entries.values()) {
    const running = entry.state === 'running';
    const own = view.streams.get(entry.stream) ?? [];
    entry.waiting = running && own.some((id) => { const held = view.items.get(id); return held?.kind === 'approval' && held.status === 'pending'; });
    entry.line = lineOf(view, entry.stream, running, t) ?? entry.line;
  }
  return [...entries.values()].sort((one, other) => one.seq - other.seq).map(({ entry }) => entry);
}

/** What the header says of the agents: how many, and a dot where one needs a person, one works, or (when none does) one failed. */
export function agentsSummary(entries: readonly AgentEntry[]): { count: number; dot: 'waiting' | 'working' | 'failed' | 'lost' | null } {
  const dot = entries.some((entry) => entry.waiting) ? 'waiting' : entries.some((entry) => entry.state === 'running') ? 'working' : entries.some((entry) => entry.state === 'failed') ? 'failed' : entries.some((entry) => entry.state === 'lost') ? 'lost' : null;
  return { count: entries.length, dot };
}

const NO_TASKS: readonly BackgroundWork[] = [];

export interface AgentHistory {
  query: string;
  setQuery: (text: string) => void;
  loading: boolean;
  error: unknown;
  hasMore: boolean;
  more: () => void;
}

/** Compact history pages keep finished children discoverable without loading their conversation bodies. */
export function useAgentCatalog(threadId: string, tasks: readonly BackgroundWork[] = NO_TASKS): { entries: AgentEntry[]; count: number | null; history: AgentHistory } {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const items = useThreadPart(threadId, (view) => view.items);
  const streams = useThreadPart(threadId, (view) => view.streams);
  const turns = useThreadPart(threadId, (view) => view.turns);
  const segments = useThreadPart(threadId, (view) => view.segments);
  const hasOlder = useThreadPart(threadId, (view) => view.hasOlder);
  const hasNewer = useThreadPart(threadId, (view) => view.hasNewer);
  const [query, setQuery] = useState('');
  const q = useDebounced(query.trim().toLocaleLowerCase(), 250);
  const remote = hasOlder || hasNewer;
  const options = outlineQuery(read, { selection: 'agents', q });
  const pages = useInfiniteQuery({ ...options, enabled: remote && options.enabled, refetchOnMount: 'always', refetchOnWindowFocus: false, retry: false });
  const local = useMemo(() => listAgents({ items, streams, turns, segments }, tasks, t), [items, streams, turns, segments, tasks, t]);
  const entries = useMemo(() => {
    const stored = pages.data?.pages.flatMap((page) => page.entries) ?? [];
    const all = listAgents({ items, streams, turns, segments }, tasks, t, remote ? stored : []);
    const matches = new Set(stored.map((point) => point.item_id));
    return q === '' ? all : all.filter((entry) => matches.has(entry.id) || `${entry.name}\n${entry.line?.text ?? ''}`.toLocaleLowerCase().includes(q));
  }, [items, streams, turns, segments, tasks, t, pages.data, q, remote]);
  const observed = useMemo(() => [...items.values()].reduce((last, item) => item.kind === 'tool_call' && item.payload.tool.kind === 'subagent' ? Math.max(last, item.seq_updated) : last, 0), [items]);
  const last = useRef(observed);
  const { refetch } = pages;
  useEffect(() => {
    if (last.current !== observed && remote) void refetch();
    last.current = observed;
  }, [observed, remote, refetch]);
  const page = pages.data?.pages[0];
  const added = page === undefined ? 0 : [...items.values()].filter((item) => item.kind === 'tool_call' && item.payload.tool.kind === 'subagent' && item.seq_created > page.seq).length;
  const count = !remote ? local.length : page?.total === undefined || page.total === null ? null : Math.max(page.total + added, local.length);
  return { entries, count, history: {
    query, setQuery, loading: remote && pages.isFetching || q !== query.trim().toLocaleLowerCase(),
    error: remote ? pages.error : null, hasMore: remote && pages.hasNextPage,
    more: () => { if (pages.data === undefined) void pages.refetch(); else void pages.fetchNextPage(); },
  } };
}
