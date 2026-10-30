/** The composer's Plan switch: the messages sent while it is on are worked in plan mode, where the agent reads,
 * proposes a plan and changes nothing until somebody proceeds. It is the person's switch, beside the approval mode
 * and apart from it: planning never changes the approval mode, and Proceed goes on under the mode the row shows.
 * It is offered only where the thread's agent program declares plan mode.
 */
import { ListChecks } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Hint } from '@/features/help/Hint';

import { useCompactControls } from '@/features/workbench/Narrow';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export function PlanSwitch({ on, onChange, unavailable }: { on: boolean; onChange: (on: boolean) => void; unavailable?: string }) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  return <span className="inline-flex min-w-0 items-center"><Tooltip content={unavailable ?? t(on ? 'composer.plan.on' : 'composer.plan.off')}>
    <Button size="sm" variant={on ? 'secondary' : 'ghost'} blocked={unavailable !== undefined} aria-pressed={on} onClick={() => onChange(!on)}
      className={cx('max-w-full', compact ? 'gap-1 px-1 text-xs' : 'gap-1 px-2 text-sm')} data-composer-plan={on ? 'on' : 'off'}>
      <ListChecks aria-hidden className="size-4 shrink-0" /><span>{t('composer.plan.label')}</span>
    </Button>
  </Tooltip><Hint id="plan-mode" popover /></span>;
}
