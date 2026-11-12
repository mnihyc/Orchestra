/** The plan of the running turn as one line above the composer.
 *
 * The plan of the running turn as one slim line above the composer: "Step 3 of 7 · Patching the
 * handler". Pressing it brings the plan's card in the conversation into view.
 */
import { ListChecks } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { runningTurn, threadReadOnly, useThreadPart } from '@/realtime/stores/thread';

export function PlanLine({ threadId, onJump }: { threadId: string; onJump: (itemId: string) => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const plan = useThreadPart(threadId, (view) => {
    if (threadReadOnly(view) || view.thread?.archived === true) return undefined;
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
  const index = underway === -1 ? steps.findIndex((step) => step.status === 'pending') : underway;
  const label = index === -1 ? t('timeline.planLine.done', { count: steps.length, number: formatNumber(steps.length, locale) })
    : `${t('timeline.planLine.step', { index: formatNumber(index + 1, locale), total: formatNumber(steps.length, locale) })}: ${steps[index]?.text ?? ''}`;
  return (
    <button
      type="button"
      onClick={() => onJump(plan.id)}
      className="flex w-full items-center gap-2 rounded-md px-3 py-1 text-left text-sm text-text-muted hover:bg-bg-muted hover:text-text"
    >
      <ListChecks aria-hidden />
      <span aria-live="polite" aria-atomic="true" className="min-w-0 flex-1 truncate">{label}</span>
    </button>
  );
}
