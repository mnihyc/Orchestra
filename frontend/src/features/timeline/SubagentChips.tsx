/** Running sub-agents named in the thread header, opening their actual turn's child lane. */
import { Bot } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { BackgroundWork } from '@/generated/api-types';
import { backgroundName } from '@/features/running/backgroundWake';

import { timelineUi } from '@/features/timeline/state';
import { recordedItem } from '@/features/timeline/historical';
import { revealView } from '@/features/workbench/openView';
import { threadReadOnly, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';

export function SubagentChips({ threadId, onReveal, tasks, mayEnd = false, onEnd }: { threadId: string; onReveal?: ((itemId: string) => void) | undefined; tasks?: readonly BackgroundWork[]; mayEnd?: boolean; onEnd?: (task: BackgroundWork) => void }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const continuation = useThreadPart(threadId, (view) => view.read?.projection?.viewed_continuation_id);
  const items = useThreadPart(threadId, (view) => view.items);
  const segments = useThreadPart(threadId, (view) => view.segments);
  const running = useMemo(() => [...items.values()].filter((item) => !recordedItem(item, segments) && item.kind === 'tool_call' && item.payload.tool.kind === 'subagent' && item.payload.tool.state === 'running' && item.status === 'in_progress'), [items, segments]);
  const turns = useThreadPart(threadId, (view) => view.turns);
  const historical = useThreadPart(threadId, threadReadOnly);
  const known = (tasks ?? []).filter((task) => task.kind === 'subagent');
  const rows = [
    ...known.map((task) => ({ key: `task:${task.id}`, itemId: task.item_id, turnId: task.turn_id, name: backgroundName(task, t), state: t(`backgroundWake.states.${task.state}`), task })),
    ...running.filter((item) => !known.some((task) => task.item_id === item.id) && (tasks === undefined || item.turn_id !== null && ['running', 'starting'].includes(turns.get(item.turn_id)?.state ?? ''))).flatMap((item) => item.kind === 'tool_call' && item.payload.tool.kind === 'subagent'
      ? [{ key: item.id, itemId: item.id, turnId: item.turn_id, name: item.payload.tool.name ?? t('timeline.step.subagentUnnamed'), state: t(historical || item.turn_id !== null && !['running', 'starting'].includes(turns.get(item.turn_id)?.state ?? '') ? 'timeline.step.recordedRunning' : 'timeline.step.subagentState.running'), task: null }] : []),
  ];
  if (rows.length === 0) return null;
  return <div role="group" aria-label={t('timeline.subagent.running')} className="flex flex-wrap items-center gap-1">
    {rows.map((row) => {
      const task = row.task;
      return <span key={row.key} className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-1" data-background-subagent={row.key}>
        <Button size="sm" variant="ghost" className="h-auto min-h-(--control-sm) max-w-full" icon={<Bot aria-hidden />} disabled={typeof row.itemId !== 'string'} aria-label={t('timeline.subagent.jump', { name: row.name })} onClick={() => {
          if (typeof row.itemId !== 'string') return;
          timelineUi.revealStep(read, row.itemId, row.turnId ?? null);
          if (onReveal !== undefined) onReveal(row.itemId);
          else revealView('subagent', { thread: threadId, item: row.itemId, ...(continuation === undefined ? {} : { continuation }) });
        }}><span className="min-w-0 wrap-anywhere whitespace-normal text-left">{row.name}</span><span className="shrink-0 text-text-subtle">{row.state}</span></Button>
        {task?.state === 'running' && mayEnd && (task.can_end && onEnd !== undefined ? <Button size="sm" variant="ghost" aria-label={t('backgroundWake.endNamed', { name: row.name })} onClick={() => onEnd(task)}>{t('running.end')}</Button>
          : <span className="min-w-0 wrap-anywhere text-xs text-text-subtle">{t('backgroundWake.cannotEnd')}</span>)}
      </span>;
    })}
  </div>;
}
