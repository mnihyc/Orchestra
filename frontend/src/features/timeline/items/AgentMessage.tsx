/** Streaming agent Markdown and final actions.
 *
 * What the agent said, without a bubble. The first thing it says in a turn follows its mark and name.
 * It is Markdown, with a caret after the text while it is still arriving. What can be done with it
 * (copy, its link, pin) is in the menu at its end, which takes no room of its own under the text.
 */
import { Bot } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { useAgentOf } from '@/features/timeline/agent';
import { useTimelineContext } from '@/features/timeline/context';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { AgentReferences } from '@/features/people/AgentReferences';
import { Avatar } from '@/ui/Avatar';
import { Markdown } from '@/ui/Markdown';
import { cx } from '@/ui/cx';

type AgentItem = Extract<Item, { kind: 'agent_message' }>;

export function AgentMark({ threadId, segmentId }: { threadId: string; segmentId: string | null }) {
  const agent = useAgentOf(threadId, segmentId);
  const name = agent?.name ?? '';
  return (
    <p className="flex items-center gap-2 text-sm">
      <Avatar kind="agent" size={24} text={Array.from(name)[0] ?? ''} color={0} mark={<Bot aria-hidden />} />
      <span className="font-medium text-text">{name}</span>
    </p>
  );
}

export function AgentMessage({ item, head }: { item: AgentItem; head: boolean }) {
  const { t } = useTranslation();
  const { threadId, compact = false } = useTimelineContext();
  const running = item.status === 'in_progress';
  return (
    <ItemFrame item={item} label={t('timeline.agent.menu')}>
      <div className={cx('flex flex-col', compact ? 'gap-1' : 'gap-2')}>
        {head && <AgentMark threadId={threadId} segmentId={item.segment_id} />}
        <AgentReferences threadId={threadId}>
          <Markdown text={item.payload.text} streaming={running} compact={compact} />
        </AgentReferences>
      </div>
    </ItemFrame>
  );
}
