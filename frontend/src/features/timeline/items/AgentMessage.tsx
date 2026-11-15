/** Streaming agent Markdown and final actions.
 *
 * What the agent said, as plain text on the page: no bubble, no mark, no name. A sentence said between steps stands
 * in quiet type among them; the last thing said in a turn that is over, its answer, carries the turn's toolbar
 * on hover. It is Markdown, with a caret after the text while it is still arriving. What can be done with it
 * (copy, its link, pin) is in the menu at its end, which takes no room of its own under the text.
 */
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { AnswerFrame } from '@/features/timeline/AnswerFrame';
import { useConversationTurn, useTimelineContext } from '@/features/timeline/context';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { AgentReferences } from '@/features/people/AgentReferences';
import { Markdown } from '@/ui/Markdown';

type AgentItem = Extract<Item, { kind: 'agent_message' }>;

/** The type of a sentence said between steps: smaller and quieter than the type the text is set in. */
export const QUIET = 'text-sm! text-text-muted!';

export function AgentMessage({ item, answer = false, quiet = false }: { item: AgentItem; answer?: boolean; quiet?: boolean }) {
  const { t } = useTranslation();
  const { threadId, compact = false } = useTimelineContext();
  const turn = useConversationTurn(answer ? item.turn_id : null);
  const running = item.status === 'in_progress';
  const said = (
    <div className="min-w-0">
      <AgentReferences threadId={threadId}>
        <Markdown text={item.payload.text} streaming={running} compact={compact || quiet} {...(quiet ? { className: QUIET } : {})} />
      </AgentReferences>
    </div>
  );
  if (turn !== undefined) return <AnswerFrame item={item} turn={turn}>{said}</AnswerFrame>;
  return <ItemFrame item={item} label={t('timeline.agent.menu')}>{said}</ItemFrame>;
}
