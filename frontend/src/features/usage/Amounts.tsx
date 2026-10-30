/** How an amount of credits is written where it stands among others: a sum with its estimate and what it leaves out, and the credits of one record with the mark of where its rate came from.
 *
 * Use that had no rate reads "–" where it stands alone and is never written as 0; a figure that is an
 * estimate carries "≈"; and a sum says beside itself how much of it is an estimate and how many tokens
 * it leaves out.
 */
import { useTranslation } from 'react-i18next';

import type { UsageEvent, UsageFigures } from '@/generated/api-types';
import { creditsState, decimal, ESTIMATE_MARK, formatCredits } from '@/features/usage/figures';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';

/** The credits of a sum, as a cell of a table says them. */
export function SumCredits({ figures, align = 'end' }: { figures: UsageFigures; align?: 'start' | 'end' }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const state = creditsState(figures);
  const side = align === 'end' ? 'items-end text-right' : 'items-start text-left';
  if (state.none) {
    return (
      <span className={`flex flex-col ${side}`}>
        <span className="text-text-muted">
          <span aria-hidden>–</span>
          <span className="sr-only">{t('usage.amounts.noRate')}</span>
        </span>
        <span className="text-xs text-text-muted">{t('usage.amounts.unratedOnly', { tokens: formatTokens(state.unrated, locale) })}</span>
      </span>
    );
  }
  return (
    <span className={`flex flex-col ${side}`}>
      <span className="tabular-nums text-text">
        {state.allEstimated && (
          <>
            <span aria-hidden>{ESTIMATE_MARK} </span>
            <span className="sr-only">{t('usage.amounts.estimate')} </span>
          </>
        )}
        {formatCredits(state.sum, locale)}
      </span>
      {state.estimated > 0 && !state.allEstimated && <span className="text-xs text-text-muted">{t('usage.amounts.estimatedPart', { amount: formatCredits(state.estimated, locale) })}</span>}
      {state.unrated > 0 && <span className="text-xs text-text-muted">{t('usage.amounts.unrated', { tokens: formatTokens(state.unrated, locale) })}</span>}
    </span>
  );
}

/** The credits of one record: "–" for use that had no rate, "≈" when the rate was an estimate by model family. */
export function EventCredits({ event }: { event: Pick<UsageEvent, 'credits' | 'rate_source'> }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (event.credits === null) {
    return (
      <span className="text-text-muted">
        <span aria-hidden>–</span>
        <span className="sr-only">{t('usage.amounts.noRate')}</span>
      </span>
    );
  }
  return (
    <span className="tabular-nums text-text">
      {event.rate_source === 'catalogue' && (
        <>
          <span aria-hidden>{ESTIMATE_MARK} </span>
          <span className="sr-only">{t('usage.amounts.estimate')} </span>
        </>
      )}
      {formatCredits(decimal(event.credits), locale)}
    </span>
  );
}
