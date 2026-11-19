/** The plan of the running turn as one line above the composer.
 *
 * The plan of the running turn as one slim line above the composer: "Step 3 of 7 · Patching the
 * handler". Pressing it brings the plan's card in the conversation into view.
 */
import { ListChecks } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { runningTurn, useThreadPart } from '@/realtime/stores/thread';

export function PlanLine({ threadId, onJump }: { threadId: string; onJump: (itemId: string) => void }) {
  const { t } = useTranslation();
  const plan = useThreadPart(threadId, (view) => {
    const turn = runningTurn(view);
    if (turn === null) return undefined;
    for (let at = view.order.length - 1; at >= 0; at -= 1) {
      const item = view.items.get(view.order[at] ?? '');
      if (item?.kind === 'plan' && item.turn_id === turn.id) return item;
    }
    return undefined;
  });
  if (plan === undefined) return null;
  const steps = plan.payload.steps;
  if (steps.length === 0) return null;
  const underway = steps.findIndex((step) => step.status === 'in_progress');
  const index = underway === -1 ? Math.min(steps.length - 1, steps.filter((step) => step.status === 'completed').length) : underway;
  return (
    <button
      type="button"
      onClick={() => onJump(plan.id)}
      className="flex w-full items-center gap-2 rounded-md px-3 py-1 text-left text-sm text-text-muted hover:bg-bg-muted hover:text-text"
    >
      <ListChecks aria-hidden />
      <span className="min-w-0 flex-1 truncate">
        {t('timeline.planLine.step', { index: index + 1, total: steps.length })}: {steps[index]?.text}
      </span>
    </button>
  );
}
