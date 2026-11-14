/** Available reasoning summaries.
 *
 * The agent's thinking, as one line: "Thought for 4 s", which opens to the summary of it in quiet
 * type. Where the upstream kept the full reasoning back, the line says so.
 */
import { ChevronDown, ChevronRight, Lock } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { QUIET } from '@/features/timeline/items/AgentMessage';
import { useTimelineContext } from '@/features/timeline/context';
import { timelineUi, useThreadUi } from '@/features/timeline/state';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { Markdown } from '@/ui/Markdown';

type ReasoningItem = Extract<Item, { kind: 'reasoning' }>;

export function Reasoning({ item }: { item: ReasoningItem }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const { threadId } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const ui = useThreadUi(threadId);
  const open = ui.steps.get(item.id) ?? false;
  const running = item.status === 'in_progress';
  const now = useRunningClock(running);
  const took = Math.max(0, (running ? now : Date.parse(item.updated_at)) - Date.parse(item.created_at));
  const { summary, hidden } = item.payload;
  const canOpen = summary.trim() !== '';
  return (
    <ItemFrame item={item} label={t('timeline.reasoning.menu')}>
      <div className="flex flex-col gap-1 text-sm text-text-muted">
        <button
          type="button"
          aria-expanded={canOpen ? open : undefined}
          disabled={!canOpen}
          onClick={() => timelineUi.setStep(read, item.id, !open)}
          className="flex w-fit items-center gap-1 rounded-md text-left hover:text-text disabled:cursor-default disabled:hover:text-text-muted"
        >
          {canOpen ? open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden /> : <span className="size-4" />}
          {running ? t('timeline.reasoning.thinking', { duration: formatSpan(took, context) }) : t('timeline.reasoning.thought', { duration: formatSpan(took, context) })}
        </button>
        {hidden && (
          <p className="ml-[calc(1rem+var(--spacing-1))] flex items-center gap-1 text-text-subtle">
            <Lock aria-hidden /> {t('timeline.reasoning.hidden')}
          </p>
        )}
        {open && canOpen && (
          <div className="ml-[calc(1rem+var(--spacing-1))] text-text-muted">
            <Markdown text={summary} streaming={running} compact className={QUIET} />
          </div>
        )}
      </div>
    </ItemFrame>
  );
}
