/** The agent's work in a turn as one group: a line that says what it came to, and the steps and sentences under it.
 *
 * Everything the agent did between a person's message and its answer, and the sentences it said between
 * the steps, is one group. Once the turn is over it is one line, drawn as a thin rule with its label, "Worked for 1m 31s ·
 * 29 commands · 3 files edited", which opens to the steps in place; while the turn is at it, the group is open as a
 * preview of the latest steps in a box of a bounded height that follows its own end. A turn that failed or was stopped
 * says so on the line of the work it ended in; a step that failed and was got past does not. What a person chose to open
 * or close holds while the screen is open.
 */
import type { TFunction } from 'i18next';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { memo, useEffect, useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import type { Item, Turn } from '@/generated/api-types';
import { useTimelineContext } from '@/features/timeline/context';
import { ItemRow } from '@/features/timeline/ItemRow';
import { isOver } from '@/features/timeline/rows';
import { timelineUi, useThreadUi } from '@/features/timeline/state';
import type { ToolItem } from '@/features/timeline/titles';
import { turnActiveMs } from '@/features/timeline/turnTime';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { type ThreadView, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { webAction } from './tools/webAction';

/** What a turn's work came to, by kind of step. */
export interface WorkSummary {
  reads: number;
  commands: number;
  /** Files changed, each counted once. */
  edits: number;
  searches: number;
  fetches: number;
  tools: number;
  subagents: number;
}

const isTool = (item: Item | undefined): item is ToolItem => item?.kind === 'tool_call';

export function summarize(items: readonly (Item | undefined)[]): WorkSummary {
  const summary: WorkSummary = { reads: 0, commands: 0, edits: 0, searches: 0, fetches: 0, tools: 0, subagents: 0 };
  const files = new Set<string>();
  for (const item of items) {
    if (!isTool(item)) continue;
    const tool = item.payload.tool;
    switch (tool.kind) {
      case 'file_read':
        summary.reads += 1;
        break;
      case 'command': {
        const action = tool.actions[0];
        if (action?.type === 'read') summary.reads += 1;
        else if (action?.type === 'search' || action?.type === 'list') summary.searches += 1;
        else summary.commands += 1;
        break;
      }
      case 'file_change':
        for (const change of tool.changes) files.add(change.path);
        break;
      case 'file_search':
        summary.searches += 1;
        break;
      case 'web_search': {
        const action = webAction(tool);
        if (action.type === 'open_page') summary.fetches += 1;
        else if (action.type === 'other') summary.tools += 1;
        else summary.searches += 1;
        break;
      }
      case 'web_fetch':
        summary.fetches += 1;
        break;
      case 'subagent':
        summary.subagents += 1;
        break;
      default:
        summary.tools += 1;
    }
  }
  summary.edits = files.size;
  return summary;
}

/** "29 commands · 3 files edited": what the work came to, in a few words. */
export function describeWork(summary: WorkSummary, t: TFunction): string {
  const parts: string[] = [];
  if (summary.commands > 0) parts.push(t('timeline.group.commands', { count: summary.commands }));
  if (summary.edits > 0) parts.push(t('timeline.group.edited', { count: summary.edits }));
  if (summary.reads > 0) parts.push(t('timeline.group.read', { count: summary.reads }));
  if (summary.searches > 0) parts.push(t('timeline.group.searched', { count: summary.searches }));
  if (summary.fetches > 0) parts.push(t('timeline.group.fetched', { count: summary.fetches }));
  if (summary.subagents > 0) parts.push(t('timeline.group.subagents', { count: summary.subagents }));
  if (summary.tools > 0) parts.push(t('timeline.group.tools', { count: summary.tools }));
  return parts.join(' · ');
}

/** The name of the agent that took a turn over from another, or null where it is the agent of the turn before. */
export function changedAgent(view: ThreadView, turn: Turn | undefined): string | null {
  if (turn === undefined) return null;
  let before: Turn | undefined;
  for (const candidate of view.turns.values()) {
    if (candidate.ordinal < turn.ordinal && (before === undefined || candidate.ordinal > before.ordinal)) before = candidate;
  }
  if (before === undefined) return null;
  const agentOf = (of: Turn) => view.segments.find((segment) => segment.id === of.segment_id)?.agent ?? view.thread?.binding?.agent ?? null;
  const now = agentOf(turn);
  const then = agentOf(before);
  return now !== null && then !== null && now.id !== then.id ? now.name : null;
}

/** How near its end the preview has to be scrolled to go on following it, in pixels. */
const FOLLOWING_SLACK_PX = 24;

interface WorkGroupProps {
  rowKey: string;
  turnId: string;
  ids: readonly string[];
  last: boolean;
  live: boolean;
  only: boolean;
}

function WorkGroupView({ rowKey, turnId, ids, last, live, only }: WorkGroupProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { threadId } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const ui = useThreadUi(threadId);
  const body = useId();
  const items = useThreadPart(threadId, useShallow((view) => ids.map((id) => view.items.get(id))));
  const turn = useThreadPart(threadId, (view) => view.turns.get(turnId));
  const agent = useThreadPart(threadId, (view) => changedAgent(view, turn));
  const open = ui.steps.get(rowKey) ?? live;

  // The preview of a turn that is at it follows its own end until its reader scrolls up in it.
  const box = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLOListElement>(null);
  const following = useRef(true);
  useEffect(() => {
    const element = box.current;
    const inner = list.current;
    if (!live || !open || element === null || inner === null) return;
    const follow = () => {
      if (following.current) element.scrollTop = element.scrollHeight;
    };
    follow();
    if (typeof ResizeObserver === 'undefined') return;
    const watch = new ResizeObserver(follow);
    watch.observe(inner);
    return () => watch.disconnect();
  }, [live, open, ids.length]);

  // How long it worked: the turn's own working time where this is all its work, else from the first step to the last.
  const present = items.flatMap((item) => item === undefined ? [] : [item]);
  const first = present[0];
  const end = present.at(-1);
  const span = first === undefined || end === undefined ? 0 : Math.max(0, Date.parse(end.updated_at) - Date.parse(first.created_at));
  const whole = only && turn !== undefined && turn.completed_at !== null ? turnActiveMs(turn, Date.parse(turn.completed_at)) : null;
  const took = live ? 0 : whole ?? span;
  const time = took >= 1000 ? formatSpan(took, format) : null;
  const ending = last && turn !== undefined && isOver(turn) ? turn.state : null;

  const head = live ? t('timeline.group.working')
    : ending === 'failed' ? (time === null ? t('timeline.group.failed') : t('timeline.group.failedAfter', { time }))
      : ending === 'interrupted' ? (time === null ? t('timeline.group.stopped') : t('timeline.group.stoppedAfter', { time }))
        : time === null ? t('timeline.group.worked') : t('timeline.group.workedFor', { time });
  const stopper = ending === 'interrupted' ? turn?.interrupted_by ?? null : null;
  const by = stopper === null ? null : t('timeline.group.stoppedBy', { name: stopper.display_name });
  const counts = describeWork(summarize(present), t);
  const label = [agent, head, by, counts === '' ? null : counts].filter((part) => part !== null).join(' · ');

  return (
    <div className="flex min-w-0 flex-col gap-2" data-work={rowKey}>
      <div className="flex min-w-0 items-center gap-3 text-sm text-text-muted">
        <span aria-hidden className="h-px min-w-4 flex-1 bg-border" />
        <button
          type="button"
          aria-expanded={open}
          aria-controls={body}
          data-fold={turnId}
          onClick={() => timelineUi.setStep(read, rowKey, !open)}
          className="flex min-h-8 min-w-0 max-w-full items-center gap-1 rounded-md px-2 hover:bg-bg-muted hover:text-text"
        >
          <span data-overflow-text={label} className="min-w-0 truncate">{label}</span>
          {open ? <ChevronUp className="shrink-0" aria-hidden /> : <ChevronDown className="shrink-0" aria-hidden />}
        </button>
        <span aria-hidden className="h-px min-w-4 flex-1 bg-border" />
      </div>
      {open && (
        <div
          id={body}
          ref={box}
          tabIndex={live ? 0 : undefined}
          onScroll={live ? (event) => {
            const element = event.currentTarget;
            following.current = element.scrollHeight - element.scrollTop - element.clientHeight < FOLLOWING_SLACK_PX;
          } : undefined}
          className={live ? 'max-h-(--work-preview-height) overflow-y-auto overscroll-contain' : undefined}
        >
          <ol ref={list} role="list" className="flex list-none flex-col gap-1">
            {ids.map((id) => <li key={id} className="min-w-0"><ItemRow id={id} quiet /></li>)}
          </ol>
        </div>
      )}
    </div>
  );
}

export const WorkGroup = memo(WorkGroupView);
