/** The time a change takes, and what a turn that is running means for it.
 *
 * "About 6–9 minutes, during which this thread stands still", marked when it rests on an assumed speed, and, when a turn
 * is running now, how long the change waits for it or that it is stopped when the person confirms. A fork stands nothing
 * still, and says only how long it takes.
 */
import { Clock } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { rangeWords } from '@/features/change/plan';
import type { AppliedChangeOptions, Estimate } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';

export interface TimeProps {
  estimate: Estimate;
  options: AppliedChangeOptions;
  turnRunning: boolean;
  /** What stands still while it goes on: "this thread", "this environment", "2 threads", or nothing for a fork. */
  stands: string | null;
}

export function Time({ estimate, options, turnRunning, stands }: TimeProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const range = rangeWords(estimate.seconds_low, estimate.seconds_high, locale);
  return (
    <div className="flex flex-col gap-1 text-base text-text" data-plan-time>
      <p className="flex items-start gap-2">
        <Clock className="mt-0.5 shrink-0 text-text-subtle" aria-hidden />
        <span>
          {stands === null ? t('change.time.about', { range }) : t('change.time.aboutStill', { range, stands })}
          {estimate.basis === 'assumed' && <span className="ml-1 text-text-muted">{t('change.time.assumed')}</span>}
        </span>
      </p>
      {turnRunning && <p className="pl-6 text-text-muted">{options.active_turn === 'wait' ? t('change.time.turnWait', { minutes: options.wait_minutes }) : t('change.time.turnStop')}</p>}
    </div>
  );
}
