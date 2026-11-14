/** One item of the conversation, drawn by its kind.
 *
 * Read from the store by its id, so that a change to one item draws that row alone. The conversation
 * draws it as a row of its own; the work of a turn draws its steps and the sentences between them
 * with it, `quiet` for those sentences; `answer` marks the last thing the agent said in a turn that is over, and `named` a
 * person's message in a thread that more than one person wrote in.
 */
import { memo } from 'react';
import { useTranslation } from 'react-i18next';

import { useTimelineContext } from '@/features/timeline/context';
import { AgentMessage } from '@/features/timeline/items/AgentMessage';
import { AnnouncementCard } from '@/features/timeline/items/Announcement';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { ErrorRow } from '@/features/timeline/items/Error';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { Plan } from '@/features/timeline/items/Plan';
import { Reasoning } from '@/features/timeline/items/Reasoning';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { useThreadPart } from '@/realtime/stores/thread';
import { InlineAlert } from '@/ui/InlineAlert';

export const ItemRow = memo(function ItemRow({ id, answer = false, named = false, quiet = false }: { id: string; answer?: boolean; named?: boolean; quiet?: boolean }) {
  const { threadId, inspection } = useTimelineContext();
  const { t } = useTranslation();
  const item = useThreadPart(threadId, (view) => view.items.get(id));
  if (item === undefined) return null;
  switch (item.kind) {
    case 'user_message':
      return <UserMessage item={item} named={named} />;
    case 'agent_message':
      return <AgentMessage item={item} answer={answer} quiet={quiet} />;
    case 'reasoning':
      return <Reasoning item={item} />;
    case 'plan':
      return <Plan item={item} />;
    case 'tool_call':
      return <ToolCallRow threadId={threadId} id={id} />;
    case 'approval':
      return inspection === undefined ? <ApprovalCard item={item} /> : <InlineAlert tone="info">{t('trace.retainedApproval')}</InlineAlert>;
    case 'announcement':
      return <AnnouncementCard item={item} />;
    case 'marker':
      return <MarkerLine item={item} />;
    case 'error':
      return <ErrorRow item={item} />;
  }
});
