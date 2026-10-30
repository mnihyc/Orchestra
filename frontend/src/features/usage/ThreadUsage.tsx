/** What bounds a thread and what it used: the tightest cap as a percentage for its chip, the caps that cover it as the rows of the Limits card with how firmly each holds there, what the thread used so far, and the way to its usage.
 *
 * The thread's own screen shows these in the popover of its usage chip and behind "View limits" on a
 * blocked thread. A cap that covers the thread is any cap on the way down to it, of the system, and of the
 * providers used inside it; the share of a cap counted per person is the reader's own. How firmly a cap
 * holds is the thread's own: within one request where the relay counts, when the agent reports where it
 * does, and not at all where nothing reports.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Gauge } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { placeLimitsQuery, usageQuery } from '@/api/usage';
import type { CapStatus, Metering, Thread } from '@/generated/api-types';
import { creditsState, decimal, ESTIMATE_MARK, formatCredits } from '@/features/usage/figures';
import { Limits } from '@/features/usage/Limits';
import { ALL_TIME_FROM, today } from '@/features/usage/search';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';

/** How full the tightest cap is, in percent of its amount, counting what turns still running have reported; none where no cap holds a count. */
export function tightestPercent(caps: readonly CapStatus[]): number | null {
  let tightest: number | null = null;
  for (const status of caps) {
    const cap = status.cap;
    if (!cap.enabled || cap.meter === 'running' || cap.amount === null) continue;
    const amount = decimal(cap.amount);
    const percent = amount <= 0 ? 100 : ((decimal(status.used) + decimal(status.in_progress)) / amount) * 100;
    if (tightest === null || percent > tightest) tightest = percent;
  }
  return tightest;
}

/** The tightest cap that covers a thread, as a percent, for its chip: nothing while it is not known or none holds a count. */
export function useTightestCap(threadId: string): number | null {
  const query = useQuery({ ...placeLimitsQuery(threadId), retry: false });
  return query.data === undefined ? null : tightestPercent(query.data.caps);
}

/** The metering of a thread: how firmly a cap holds there. */
export function meteringOf(thread: Pick<Thread, 'binding'>): Metering | undefined {
  return thread.binding?.isolation.metering;
}

/** What a thread used so far, in tokens and credits. */
function UsedSoFar({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const now = useNow();
  const query = useQuery({ ...usageQuery({ scope: `node:${threadId}`, from: ALL_TIME_FROM, to: today(now), bucket: 'month', breakdowns: [], filters: {} }), retry: false });
  if (query.data === undefined) return null;
  const totals = query.data.totals;
  const credits = creditsState(totals);
  return (
    <p className="text-sm text-text-muted">
      {t('caps.thread.used', {
        tokens: formatTokens(totals.tokens.total_tokens, locale),
        credits: credits.none ? t('caps.thread.noRate') : `${credits.allEstimated ? `${ESTIMATE_MARK} ` : ''}${formatCredits(credits.sum, locale)}`,
      })}
    </p>
  );
}

/** What stands in the popover of a thread's usage chip below the provider's windows: the caps that cover it, and what it used. */
export function ThreadUsage({ thread }: { thread: Pick<Thread, 'id' | 'name' | 'binding'> }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2 border-t border-border pt-2" data-tutorial-target="thread-usage">
      <UsedSoFar threadId={thread.id} />
      {/* A thread under many limits would run the card off the screen: the limits scroll, and the way to the details stays where it is. */}
      <div className="max-h-[20rem] overflow-y-auto">
        <Limits of={{ kind: 'node', id: thread.id, name: thread.name, manage: false }} metering={meteringOf(thread)} bare />
      </div>
      <Link to="/usage" search={{ scope: `node:${thread.id}`, range: 'all' }} className="w-fit text-sm text-accent-text hover:underline">
        {t('caps.thread.details')}
      </Link>
    </div>
  );
}

/** "View limits" on a blocked thread: what bounds it, in a card, with each limit changed in its row by whoever may. */
export function ViewLimits({ thread }: { thread: Pick<Thread, 'id' | 'name' | 'binding'> }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="sm" icon={<Gauge aria-hidden />} onClick={() => setOpen(true)}>
        {t('caps.thread.view')}
      </Button>
      <Dialog open={open} onOpenChange={setOpen} title={t('caps.thread.viewTitle')} description={t('caps.thread.viewAbout')} size="lg">
        <Limits of={{ kind: 'node', id: thread.id, name: thread.name, manage: false }} metering={meteringOf(thread)} bare />
        <p className="py-3 text-sm">
          <Link to="/usage" search={{ scope: `node:${thread.id}`, range: 'all' }} className="text-accent-text hover:underline" onClick={() => setOpen(false)}>
            {t('caps.thread.details')}
          </Link>
        </p>
      </Dialog>
    </>
  );
}
