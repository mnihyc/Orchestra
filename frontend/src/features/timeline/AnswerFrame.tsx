/** The answer of a turn that is over, with the toolbar of the turn on hover and on focus.
 *
 * Copy, try again, fork and restore the files stand in the toolbar, then the time the answer was made, which opens
 * what the turn used; the rest of the acts on the answer and on its turn are in the menu at the end of the toolbar.
 */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { Item, Turn } from '@/generated/api-types';
import { RestoreFromTurn } from '@/features/source/Restore';
import { useTimelineContext } from '@/features/timeline/context';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { useTurnActs, useTurnScope } from '@/features/timeline/TurnActions';
import { useTurnChanges } from '@/features/timeline/TurnFiles';
import { TurnFigures } from '@/features/timeline/TurnFigures';
import { CopyButton } from '@/ui/CopyButton';

type AgentItem = Extract<Item, { kind: 'agent_message' }>;

export function AnswerFrame({ item, turn, children }: { item: AgentItem; turn: Turn; children: ReactNode }) {
  const { t } = useTranslation();
  const context = useTimelineContext();
  const scope = useTurnScope(context.threadId, turn.id);
  const acts = useTurnActs(turn, scope);
  const changes = useTurnChanges(context.threadId, turn.id);
  const hasChanges = changes.files.length > 0 || changes.reported > 0;
  const [figuresOpen, setFiguresOpen] = useState(false);
  const extra = turn.usage === null ? acts.extra : [...acts.extra, { id: 'turn-figures',
    label: t('statusItems.items.usage'), onSelect: () => setFiguresOpen(true) }];
  return (
    <ItemFrame item={item} label={t('timeline.agent.menu')} extra={extra} answerTurn={turn} restoreChanges={hasChanges} messageActions={<>
      <CopyButton value={item.payload.text} label={t('timeline.actions.copy')} />
      {acts.interactive && <>
        {acts.retry}
        {acts.fork}
        <RestoreFromTurn threadId={context.threadId} turn={turn} compact fallback hasChanges={hasChanges} readOnly={context.readOnly === true} />
      </>}
      <TurnFigures threadId={context.threadId} turn={turn} at={item.updated_at} disclosure={{ open: figuresOpen, onOpenChange: setFiguresOpen }} />
    </>}>
      {children}
      {acts.after}
    </ItemFrame>
  );
}
