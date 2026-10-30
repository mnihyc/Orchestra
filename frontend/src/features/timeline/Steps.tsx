/** The agent's work as steps: the turn that runs and the last finished one step by step, older turns folded to a line; and the depth a person chose (Steps, Summary, Everything), kept for them.
 *
 * Turns whose steps are not shown one by one stand as one line: "Read 4 files, ran 3 commands, edited
 * 2 files · 12 s", with a danger mark when a step failed, and the line opens to the steps. While the
 * turn runs, the line names the step in hand with its timer. The depth a reader chose (Steps, Summary
 * or Everything) decides which turns fold; the choice is theirs and follows them to every device.
 */
import type { TFunction } from 'i18next';
import { ChevronDown, ChevronRight, LoaderCircle, OctagonAlert } from 'lucide-react';
import { memo } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import type { Item } from '@/generated/api-types';
import { useTimelineContext } from '@/features/timeline/context';
import { timelineUi } from '@/features/timeline/state';
import { describeStep, stepFailed, type ToolItem } from '@/features/timeline/titles';
import { turnActiveMs, turnWaiting } from '@/features/timeline/turnTime';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { cx } from '@/ui/cx';

/** What a turn's work came to, by kind of step. */
export interface WorkSummary {
  reads: number;
  commands: number;
  edits: number;
  searches: number;
  fetches: number;
  tools: number;
  subagents: number;
  failed: boolean;
  /** The step that is under way, if one is. */
  current: ToolItem | null;
  /** The time the steps took, added together. */
  stepMs: number;
}

const isTool = (item: Item | undefined): item is ToolItem => item?.kind === 'tool_call';

export function summarize(items: readonly (Item | undefined)[]): WorkSummary {
  const summary: WorkSummary = { reads: 0, commands: 0, edits: 0, searches: 0, fetches: 0, tools: 0, subagents: 0, failed: false, current: null, stepMs: 0 };
  for (const item of items) {
    if (!isTool(item)) continue;
    const tool = item.payload.tool;
    if (stepFailed(item)) summary.failed = true;
    if (item.status === 'in_progress') summary.current = item;
    summary.stepMs += item.payload.duration_ms ?? 0;
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
        summary.edits += tool.changes.length;
        break;
      case 'file_search':
      case 'web_search':
        summary.searches += 1;
        break;
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
  return summary;
}

/** "Read 4 files, ran 3 commands, edited 2 files". */
export function describeWork(summary: WorkSummary, t: TFunction): string {
  const parts: string[] = [];
  if (summary.reads > 0) parts.push(t('timeline.work.read', { count: summary.reads }));
  if (summary.searches > 0) parts.push(t('timeline.work.searched', { count: summary.searches }));
  if (summary.commands > 0) parts.push(t('timeline.work.ran', { count: summary.commands }));
  if (summary.edits > 0) parts.push(t('timeline.work.edited', { count: summary.edits }));
  if (summary.fetches > 0) parts.push(t('timeline.work.fetched', { count: summary.fetches }));
  if (summary.subagents > 0) parts.push(t('timeline.work.subagents', { count: summary.subagents }));
  if (summary.tools > 0) parts.push(t('timeline.work.tools', { count: summary.tools }));
  const text = parts.join(t('timeline.work.separator'));
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function WorkFoldView({ turnId, ids, open }: { turnId: string; ids: readonly string[]; open: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { threadId, readOnly } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const items = useThreadPart(threadId, useShallow((view) => ids.map((id) => view.items.get(id))));
  const turn = useThreadPart(threadId, (view) => view.turns.get(turnId));
  const summary = summarize(items);
  const running = readOnly !== true && turn !== undefined && (turn.state === 'running' || turn.state === 'starting');
  const active = running && !turnWaiting(turn);
  const now = useRunningClock(active);
  const durationMs = turn === undefined ? summary.stepMs : turnActiveMs(turn, now) ?? summary.stepMs;
  const current = summary.current;
  const line =
    active && current !== null
      ? `${describeStep(current, t).text} (${formatSpan(durationMs, format)})`
      : `${describeWork(summary, t)}${durationMs > 0 ? ` (${formatSpan(durationMs, format)})` : ''}`;
  return (
    <button
      type="button"
      aria-expanded={open}
      data-fold={turnId}
      onClick={() => timelineUi.toggleFold(read, turnId)}
      className={cx('flex min-h-8 w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm text-text-muted hover:bg-bg-muted hover:text-text', summary.failed && 'text-danger-text')}
    >
      {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
      {active ? <LoaderCircle className="animate-spin text-info" aria-hidden /> : summary.failed ? <OctagonAlert className="text-danger" aria-hidden /> : null}
      <span className="min-w-0 flex-1 truncate">{line === '' ? t('timeline.work.nothing') : line}</span>
      {summary.failed && <span className="sr-only">{t('timeline.step.failed')}</span>}
    </button>
  );
}

export const WorkFold = memo(WorkFoldView);
