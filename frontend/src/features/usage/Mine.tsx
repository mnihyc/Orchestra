/** One's own usage this month in three figures with a small chart that leads to the usage screen, and the share of each allowance that covers one.
 *
 * What a person used themself, which they read in full whatever a place lets them see of others, and
 * what limits them: every cap that is counted for each person and covers them, with what they have used of
 * it. It is the content of the personal page "Usage and limits"; its frame is the page's own.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { usageQuery } from '@/api/usage';
import { boardSlots, boardSpan, MiniBars } from '@/features/usage/Board';
import { Figures } from '@/features/usage/Figures';
import { Limits } from '@/features/usage/Limits';
import { useNow } from '@/lib/time';
import { boxClasses, Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { Skeleton } from '@/ui/Skeleton';

export function OwnUsage() {
  const { t } = useTranslation();
  const now = useNow();
  const span = boardSpan('month', now);
  const query = useQuery(usageQuery({ scope: 'me', from: span.from, to: span.to, breakdowns: [], filters: {} }));
  const analytics = query.data;
  return (
    <div className="flex flex-col gap-6">
      <Section
        id="own-usage"
        title={t('usage.mine.title')}
        lead={t('usage.mine.lead')}
        action={
          <Link to="/usage" className="text-sm text-accent-text hover:underline">
            {t('usage.mine.open')}
          </Link>
        }
      >
        {query.isPending ? (
          <Skeleton className="h-[6rem] w-full rounded-lg" />
        ) : query.isError ? (
          <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
        ) : analytics === undefined ? null : (
          <div className="flex flex-col gap-3">
            <Figures totals={analytics.totals} previous={null} before="" />
            {analytics.totals.turns > 0 && (
              <div className={`${boxClasses} p-4`}>
                <MiniBars analytics={analytics} label={t('usage.board.byBucket', { bucket: t(`usage.bucket.${analytics.bucket}`) })} slots={boardSlots('month', now)} />
              </div>
            )}
          </div>
        )}
      </Section>
      <Section id="own-allowances" title={t('usage.mine.allowances')}>
        <Limits of={{ kind: 'mine' }} />
      </Section>
    </div>
  );
}
