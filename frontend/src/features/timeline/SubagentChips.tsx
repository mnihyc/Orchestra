/** Running sub-agents named in the thread header, opening their actual turn's child lane. */
import { Bot } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { timelineUi } from '@/features/timeline/state';
import { recordedItem } from '@/features/timeline/historical';
import { revealView } from '@/features/workbench/openView';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';

export function SubagentChips({ threadId, onReveal }: { threadId: string; onReveal?: ((itemId: string) => void) | undefined }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const continuation = useThreadPart(threadId, (view) => view.read?.projection?.viewed_continuation_id);
  const items = useThreadPart(threadId, (view) => view.items);
  const segments = useThreadPart(threadId, (view) => view.segments);
  const running = useMemo(() => [...items.values()].filter((item) => !recordedItem(item, segments) && item.kind === 'tool_call' && item.payload.tool.kind === 'subagent' && item.payload.tool.state === 'running' && item.status === 'in_progress'), [items, segments]);
  if (running.length === 0) return null;
  return <div role="group" aria-label={t('timeline.subagent.running')} className="flex flex-wrap items-center gap-1">
    {running.map((item) => {
      if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'subagent') return null;
      const name = item.payload.tool.name ?? t('timeline.step.subagentUnnamed');
      return <Button key={item.id} size="sm" variant="ghost" className="h-auto min-h-(--control-sm) max-w-full" icon={<Bot aria-hidden />} aria-label={t('timeline.subagent.jump', { name })} onClick={() => {
        timelineUi.revealStep(read, item.id, item.turn_id);
        if (onReveal !== undefined) onReveal(item.id);
        else revealView('subagent', { thread: threadId, item: item.id, ...(continuation === undefined ? {} : { continuation }) });
      }}><span className="min-w-0 wrap-anywhere whitespace-normal text-left">{name}</span><span className="shrink-0 text-text-subtle">{t('timeline.step.subagentState.running')}</span></Button>;
    })}
  </div>;
}
