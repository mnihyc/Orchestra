/** A sub-agent in its parent's work: one compact row, the way its program draws it.
 *
 * The row names the sub-agent by its kind and its task (`Explore · Find the login flow`) and says how it stands and how
 * long it has run; one quiet line under it gives its figures (how many tools it has used, the model where its program
 * names one) and what it is doing now while it runs, or what it came to once it is over. Several started at once stand one under
 * the other, each with its own line. Selecting the row shows the sub-agent's own conversation in place of the main
 * one where the screen of a thread can do that, and otherwise opens it under the row; "Open in panel" beside it
 * opens it next to the conversation.
 */
import { Braces, Bot, ChevronDown, ChevronRight, PanelRightOpen } from 'lucide-react';
import { memo } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { useAgentSwitch } from '@/features/thread/agentSwitch';
import { useTimelineContext } from '@/features/timeline/context';
import { useRecordedItem } from '@/features/timeline/historical';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { StepState } from '@/features/timeline/items/StepState';
import { Display, useRawCopies } from '@/features/timeline/items/ToolCall';
import { RawCall } from '@/features/timeline/RawCall';
import { timelineUi, useThreadUi } from '@/features/timeline/state';
import { subagentLine, subagentTask, subagentToolCount } from '@/features/timeline/subagents';
import type { ToolItem } from '@/features/timeline/titles';
import { revealView } from '@/features/workbench/openView';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan, useRunningClock } from '@/lib/time';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { IconButton } from '@/ui/IconButton';
import { cx } from '@/ui/cx';

function SubagentRowView({ item }: { item: ToolItem }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const context = useTimelineContext();
  const switchTo = useAgentSwitch();
  const read = useThreadReadRef(context.threadId);
  const continuation = useThreadPart(context.threadId, (view) => view.read?.projection?.viewed_continuation_id);
  const recorded = useRecordedItem(context.threadId, item);
  const ui = useThreadUi(context.threadId);
  const tool = item.payload.tool;
  const stream = tool.kind === 'subagent' ? tool.child_stream : '';
  const running = item.status === 'in_progress' && !recorded;
  const failed = item.status === 'failed' || tool.kind === 'subagent' && tool.state === 'failed';
  // What it does now or came to, and how many tools it used, from its own items as they arrive.
  const [tools, lineKind, lineText] = useThreadPart(context.threadId, useShallow((view) => {
    const line = subagentLine(view, stream, running, t);
    return [subagentToolCount(view, stream), line?.kind ?? null, line?.text ?? null] as const;
  }));
  const now = useRunningClock(running);
  // Selecting it shows its conversation in place of the main one; where that cannot be, the row opens it under itself.
  const switching = switchTo !== null && stream !== '';
  const open = ui.steps.get(item.id) ?? false;
  const raw = ui.raw.has(item.id);
  const copies = useRawCopies(item, open);
  if (tool.kind !== 'subagent') return null;
  const tookMs = running ? Math.max(0, now - Date.parse(item.payload.started_at)) : item.payload.duration_ms;
  const took = tookMs === null || tookMs < 1000 ? null : formatSpan(tookMs, format);
  const role = tool.role === null || tool.role.trim() === '' ? null : tool.role.trim();
  const named = subagentTask(tool);
  const task = named ?? (role === null ? t('timeline.step.subagentUnnamed') : null);
  const model = tool.model === undefined || tool.model === null || tool.model === '' ? null : tool.model;
  // Its figures lead the line under it, the way its program says them: "2 tool uses · haiku".
  const figures = [tools === null || tools === 0 ? null : t('timeline.subagent.tools', { count: tools, number: formatNumber(tools, format.locale) }), model]
    .filter((part) => part !== null).join(' · ');
  const shownInline = !switching && open || open && raw;
  const stateWord = recorded && item.status === 'in_progress' ? t('timeline.step.recordedRunning') : t(`timeline.step.subagentState.${tool.state}`);
  // Read aloud as one line: what it is, how it stands, its figures and its line.
  const head = named !== null || role !== null ? t('timeline.step.subagent', { name: named ?? role }) : t('timeline.step.subagentUnnamed');
  const label = [head, named === null ? null : role, stateWord, took, figures === '' ? null : figures, lineText]
    .filter((part) => part !== null && part !== '').join(' · ');

  return (
    <ItemFrame item={item} label={t('timeline.step.menu')} extra={copies.entries}>
      <div className="flex flex-col gap-1" data-step={item.id} data-subagent-row={item.id}>
        <div className="flex min-w-0 items-start gap-1 pr-[calc(var(--control-sm)+var(--spacing-3))]" data-tool-row>
          <button
            type="button"
            aria-label={label}
            aria-expanded={switching ? undefined : open}
            onClick={() => { if (switching) switchTo(item.id); else timelineUi.setStep(read, item.id, !open); }}
            className={cx('flex min-h-8 min-w-0 flex-1 items-start gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-bg-muted', failed && 'text-danger-text')}
          >
            <span className="inline-flex h-5 shrink-0 items-center gap-2">
              {!switching && (open ? <ChevronDown className="text-text-subtle" aria-hidden /> : <ChevronRight className="text-text-subtle" aria-hidden />)}
              <Bot className={failed ? 'text-danger' : 'text-text-muted'} aria-hidden />
            </span>
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="flex min-w-0 items-baseline gap-2">
                {role !== null && <span data-subagent-role data-overflow-text={role} className="max-w-1/3 shrink-0 truncate font-medium text-text">{role}</span>}
                {task !== null && <span data-subagent-task data-overflow-text={task} className={cx('min-w-0 truncate', !failed && 'text-text')}>{task}</span>}
              </span>
              {(figures !== '' || lineText !== null) && <span data-subagent-summary={lineKind ?? undefined} data-overflow-text={[figures, lineText].filter((part) => part !== '' && part !== null).join(' · ')}
                className="truncate text-xs text-text-muted">
                {figures !== '' && <span data-subagent-figures className="tabular-nums">{figures}</span>}
                {figures !== '' && lineText !== null && <span aria-hidden> · </span>}
                {lineText}
              </span>}
            </span>
            <span className="inline-flex h-5 shrink-0 items-center">
              <StepState item={item} failed={failed} took={took} recorded={recorded} cancelled={t('timeline.group.stopped')} />
            </span>
          </button>
          {open && raw && <IconButton size="sm" icon={<Braces aria-hidden />} label={t('timeline.step.showDisplay')} pressed onClick={() => timelineUi.toggleRaw(read, item.id)} />}
          {context.inspection === undefined && <IconButton size="sm" icon={<PanelRightOpen aria-hidden />} label={t('timeline.subagent.openPanel')} blocked={stream === ''} reason={stream === '' ? t('timeline.subagent.unavailable') : undefined}
            onClick={() => revealView('subagent', { thread: context.threadId, item: item.id, ...(continuation === undefined ? {} : { continuation }) })} />}
        </div>
        {copies.feedback}
        {shownInline && (
          <div className={cx('ml-4 flex flex-col gap-2', raw && 'border-l border-border pl-3')}>
            {raw ? <RawCall item={item} /> : <Display item={item} />}
          </div>
        )}
      </div>
    </ItemFrame>
  );
}

export const SubagentRow = memo(SubagentRowView);
