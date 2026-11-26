/** What a sub-agent's row in its parent's work and its entry in the agents panel say of it, read from its own items.
 *
 * A sub-agent is told the way its program tells it: its kind and its task (`Explore · Find the login flow`), how many
 * tools it has used, how long it has run, the model where its program names one, and one line under it, what it does
 * now while it runs or what it came to once it is over. The line is plain words: the Markdown marks of a reply (a
 * heading's hashes, a list's bullet, emphasis, code ticks) are left out, since a line in a row cannot draw them.
 */
import type { TFunction } from 'i18next';

import { approvalSnapshot } from '@/features/approvals/snapshot';
import type { Item } from '@/generated/api-types';
import type { ThreadView } from '@/realtime/stores/thread';
import { firstSummaryLine } from './thinking';
import { describeStep } from './titles';

type ToolItem = Extract<Item, { kind: 'tool_call' }>;
type ApprovalItem = Extract<Item, { kind: 'approval' }>;
export type SubagentTool = Extract<ToolItem['payload']['tool'], { kind: 'subagent' }>;

/** One line of a sub-agent: what it is doing now, or what it came to. */
export interface SubagentLine { kind: 'activity' | 'result'; text: string }

/** The first line of a reply that says something, in plain words (the summary line's rule). */
export function plainLine(text: string): string {
  return firstSummaryLine(text) ?? '';
}

/** The task a sub-agent was given, as one line: the name its program gave it, else the first line of what it was asked. */
export function subagentTask(tool: SubagentTool): string | null {
  const name = tool.name?.trim() ?? '';
  if (name !== '') return name;
  const asked = plainLine(tool.prompt ?? '');
  return asked === '' ? null : asked;
}

/** How many tools a sub-agent has used, counted from its own items; `null` where the page may not hold all of them. */
export function subagentToolCount(view: Pick<ThreadView, 'items' | 'streams' | 'hasNewer'>, stream: string): number | null {
  // Its items follow the step that started it, so a window that reaches the end of the thread holds every one of them.
  if (stream === '' || view.hasNewer) return null;
  let count = 0;
  for (const id of view.streams.get(stream) ?? []) if (view.items.get(id)?.kind === 'tool_call') count += 1;
  return count;
}

/** What a sub-agent does now (while it runs) or what it came to (once it is over), in one line of its own items. */
export function subagentLine(view: Pick<ThreadView, 'items' | 'streams'>, stream: string, running: boolean, t: TFunction): SubagentLine | null {
  const ids = view.streams.get(stream) ?? [];
  for (let at = ids.length - 1; at >= 0; at -= 1) {
    const item = view.items.get(ids[at] ?? '');
    if (item === undefined) continue;
    if (!running) {
      if (item.kind === 'agent_message') {
        const text = plainLine(item.payload.text);
        if (text !== '') return { kind: 'result', text };
      }
      continue;
    }
    if (item.kind === 'tool_call') return { kind: 'activity', text: describeStep(item, t).text };
    if (item.kind === 'reasoning') return { kind: 'activity', text: t('thread.now.thinking') };
    if (item.kind === 'approval' && item.status === 'pending') return { kind: 'activity', text: t('thread.now.approval') };
    if (item.kind === 'agent_message') {
      const text = plainLine(item.payload.text);
      if (text !== '') return { kind: 'activity', text };
    }
  }
  return null;
}

/**
 * Which sub-agent asks an approval: the program files every request beside the main conversation, so the request is
 * matched with the step it is about, a command or a change of files of that same turn in a sub-agent's own stream.
 * `null` where it is the main agent's, or nothing says.
 */
export function approvalStream(view: Pick<ThreadView, 'items' | 'streams'>, item: ApprovalItem): string | null {
  const approval = approvalSnapshot(item.payload.approval, item.payload.approval_id);
  if (approval?.kind !== 'harness') return null;
  const request = approval.request;
  const about = (step: ToolItem): boolean => {
    const tool = step.payload.tool;
    if (request.request_kind === 'command') return tool.kind === 'command' && tool.command.trim() !== '' && tool.command === request.detail.command;
    if (request.request_kind === 'file_change') return tool.kind === 'file_change' && request.detail.changes.some((asked) => tool.changes.some((change) => change.path === asked.path));
    return false;
  };
  for (const [stream, ids] of view.streams) {
    if (stream === 'main') continue;
    for (const id of ids) {
      const step = view.items.get(id);
      if (step?.kind === 'tool_call' && step.turn_id === item.turn_id && about(step)) return stream;
    }
  }
  return null;
}

/** The step that started the sub-agent whose own stream is `stream`. */
export function subagentOf(view: Pick<ThreadView, 'items'>, stream: string): ToolItem | null {
  for (const item of view.items.values()) {
    if (item.kind === 'tool_call' && item.payload.tool.kind === 'subagent' && item.payload.tool.child_stream === stream) return item;
  }
  return null;
}

/** The approvals a sub-agent asked, kept beside the main conversation: they belong to its own conversation too. */
export function subagentApprovals(view: Pick<ThreadView, 'items' | 'streams' | 'order'>, stream: string): string[] {
  const own = view.streams.get(stream) ?? [];
  if (own.length === 0) return [];
  const turns = new Set(own.flatMap((id) => { const turn = view.items.get(id)?.turn_id; return turn === null || turn === undefined ? [] : [turn]; }));
  return view.order.filter((id) => {
    const item = view.items.get(id);
    return item?.kind === 'approval' && item.turn_id !== null && turns.has(item.turn_id) && approvalStream(view, item) === stream;
  });
}

