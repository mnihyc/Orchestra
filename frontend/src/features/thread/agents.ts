/** The sub-agents of a thread, running and finished, read from the steps that started them.
 *
 * Each is one entry: its name, where it stands, how long it has run, what it used where that was recorded, and one line of what it is
 * doing or what it came to. A finished one stays in the list. The entry is read from the rows the page holds; the background roster
 * adds only whether the running one may be stopped.
 */
import type { TFunction } from 'i18next';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { itemText } from '@/features/timeline/ItemActions';
import { recordedItem } from '@/features/timeline/historical';
import { isOver } from '@/features/timeline/rows';
import { describeStep } from '@/features/timeline/titles';
import type { BackgroundWork, SubagentState } from '@/generated/api-types';
import { type ThreadView, useThreadPart } from '@/realtime/stores/thread';

export interface AgentEntry {
  /** The step that started it. */
  id: string;
  /** Where its own items are. */
  stream: string;
  turnId: string | null;
  name: string;
  /** `recorded`: a session that has ended marked it as running; nothing says how it came out. */
  state: SubagentState | 'recorded';
  startedAt: string;
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

export function listAgents(view: Pick<ThreadView, 'items' | 'streams' | 'turns' | 'segments'>, tasks: readonly BackgroundWork[], t: TFunction): AgentEntry[] {
  const entries: { seq: number; entry: AgentEntry }[] = [];
  for (const item of view.items.values()) {
    if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'subagent') continue;
    const tool = item.payload.tool;
    const turn = item.turn_id === null ? undefined : view.turns.get(item.turn_id);
    const recorded = tool.state === 'running' && (recordedItem(item, view.segments) || turn !== undefined && isOver(turn));
    const state = recorded ? 'recorded' : tool.state;
    const running = state === 'running';
    const stream = tool.child_stream;
    const own = stream === '' ? [] : view.streams.get(stream) ?? [];
    const output = turn?.usage_detail?.by_subagent.find((share) => share.stream === stream)?.tokens.output_tokens ?? 0;
    entries.push({ seq: item.seq_created, entry: {
      id: item.id, stream, turnId: item.turn_id,
      name: tool.name !== null && tool.name !== '' ? tool.name : t('timeline.step.subagentUnnamed'),
      state, startedAt: item.payload.started_at, durationMs: item.payload.duration_ms,
      tokensOut: output > 0 ? output : null,
      waiting: running && own.some((id) => { const held = view.items.get(id); return held?.kind === 'approval' && held.status === 'pending'; }),
      line: lineOf(view, stream, running, t),
      task: running ? tasks.find((task) => task.kind === 'subagent' && task.item_id === item.id && task.state === 'running') ?? null : null,
    } });
  }
  return entries.sort((one, other) => one.seq - other.seq).map(({ entry }) => entry);
}

/** What the header says of the agents: how many, and a dot where one needs a person, one works, or (when none does) one failed. */
export function agentsSummary(entries: readonly AgentEntry[]): { count: number; dot: 'waiting' | 'working' | 'failed' | null } {
  const dot = entries.some((entry) => entry.waiting) ? 'waiting' : entries.some((entry) => entry.state === 'running') ? 'working' : entries.some((entry) => entry.state === 'failed') ? 'failed' : null;
  return { count: entries.length, dot };
}

const NO_TASKS: readonly BackgroundWork[] = [];

/** The agents of a thread as the page holds them now. */
export function useAgents(threadId: string, tasks: readonly BackgroundWork[] = NO_TASKS): AgentEntry[] {
  const { t } = useTranslation();
  const items = useThreadPart(threadId, (view) => view.items);
  const streams = useThreadPart(threadId, (view) => view.streams);
  const turns = useThreadPart(threadId, (view) => view.turns);
  const segments = useThreadPart(threadId, (view) => view.segments);
  return useMemo(() => listAgents({ items, streams, turns, segments }, tasks, t), [items, streams, turns, segments, tasks, t]);
}
