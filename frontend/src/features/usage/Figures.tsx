/** The three figures of a span (turns, tokens, credits) each against the span before, what the credits are made of, and the tokens divided the way a rate divides them.
 *
 * Credits say two things about themselves wherever they are a sum: how much of them the shipped
 * catalogue estimated, and how much use they leave out for want of any rate. A change against the
 * span before is written in words and with a mark, never by colour: more use is neither good nor bad.
 */
import { Minus, TrendingDown, TrendingUp } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { UsageFigures } from '@/generated/api-types';
import { type Change, changeOf, creditsState, ESTIMATE_MARK, formatCredits, measureOf } from '@/features/usage/figures';
import type { Measure } from '@/features/usage/search';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber, formatPercent, formatTokens } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ReportFigure } from '@/features/usage/ReportFrame';
import { cacheHitRate, costSegments, COST_LOOKS } from './context/fill';
import { PartsBar, Swatch } from './context/PartsBar';
import { TOKEN_KINDS } from './series';

export function ChangeLine({ change, before }: { change: Change; before: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (change.kind === 'none') return null;
  const words =
    change.kind === 'new'
      ? t('usage.figures.changeNew', { before })
      : change.kind === 'same'
        ? t('usage.figures.changeSame', { before })
        : change.kind === 'up'
          ? t('usage.figures.changeUp', { percent: formatNumber(change.percent, locale), before })
          : t('usage.figures.changeDown', { percent: formatNumber(change.percent, locale), before });
  const Mark = change.kind === 'up' || change.kind === 'new' ? TrendingUp : change.kind === 'down' ? TrendingDown : Minus;
  return (
    <p className="flex items-center gap-1 text-sm text-text-muted">
      <Mark aria-hidden className="size-4 shrink-0" />
      <span>{words}</span>
    </p>
  );
}

function Tile({ label, value, change, before, children, className }: { label: string; value: string; change: Change; before: string; children?: ReactNode; className?: string }) {
  return (
    <ReportFigure label={label} value={value} className={className}>
      <ChangeLine change={change} before={before} />
      {children}
    </ReportFigure>
  );
}

export interface FiguresProps {
  totals: UsageFigures;
  /** The span before, or none where there is not one (all time). */
  previous: UsageFigures | null;
  /** What the span before is called in "against the 30 days before". */
  before: string;
  /** Opens the rates of the place, for whoever may set them. */
  onRates?: (() => void) | undefined;
}

export function Figures({ totals, previous, before, onRates }: FiguresProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const credits = creditsState(totals);
  const measures: Record<Extract<Measure, 'turns' | 'tokens' | 'credits'>, Change> = {
    turns: changeOf(measureOf(totals, 'turns'), previous === null ? null : measureOf(previous, 'turns')),
    tokens: changeOf(measureOf(totals, 'tokens'), previous === null ? null : measureOf(previous, 'tokens')),
    credits: changeOf(measureOf(totals, 'credits'), previous === null ? null : measureOf(previous, 'credits')),
  };
  return (
    <ul aria-label={t('usage.figures.label')} className="grid grid-cols-1 gap-3 sm:grid-cols-3">
      <Tile label={t('usage.figures.turns')} value={formatNumber(totals.turns, locale)} change={measures.turns} before={before}><p className="text-sm text-text-muted">{t('usage.figures.requests',{count:totals.performance?.requests ?? 0,value:formatNumber(totals.performance?.requests ?? 0,locale)})}</p></Tile>
      <Tile label={t('usage.figures.tokens')} value={formatTokens(totals.tokens.total_tokens, locale)} change={measures.tokens} before={before} />
      <Tile
        label={t('usage.figures.credits')}
        value={credits.none ? '–' : `${credits.allEstimated ? `${ESTIMATE_MARK} ` : ''}${formatCredits(credits.sum, locale)}`}
        change={credits.none ? { kind: 'none' } : measures.credits}
        before={before}
      >
        {credits.estimated > 0 && !credits.allEstimated && <p className="text-sm text-text-muted">{t('usage.figures.estimated', { amount: formatCredits(credits.estimated, locale) })}</p>}
        {credits.allEstimated && <p className="text-sm text-text-muted">{t('usage.figures.allEstimated')}</p>}
        {credits.unrated > 0 && (
          <p className="flex flex-wrap items-baseline gap-x-2 text-sm text-text-muted">
            <span>{t('usage.figures.unrated', { tokens: formatTokens(credits.unrated, locale) })}</span>
            {onRates !== undefined && (
              <Button size="sm" variant="ghost" onClick={onRates}>
                {t('usage.figures.addRates')}
              </Button>
            )}
          </p>
        )}
      </Tile>
    </ul>
  );
}

/** The five disjoint token kinds directly below the span's figures, with the input's cache hit rate. */
export function TokenLine({ tokens }: { tokens: UsageFigures['tokens'] }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const segments=costSegments(tokens,null);
  const hit=cacheHitRate(tokens);
  if(segments.length===0) return null;
  return <section aria-label={t('usage.tokens.label')} className="flex flex-col gap-2">
    <PartsBar segments={segments} size="thin" label={t('usage.tokens.label')} describe={(part)=>`${t(`contextUsage.cost.name.${part.key}`)} ${formatTokens(part.tokens,locale)} (${formatPercent(part.share,locale)})`} />
    <ul className="flex flex-wrap gap-x-5 gap-y-1 text-sm text-text-muted">
      {TOKEN_KINDS.map((kind)=><li key={kind} className="flex items-center gap-2"><Swatch segment={{look:COST_LOOKS[kind]}} /><span>{t(`contextUsage.cost.name.${kind}`)} <span className="text-text tabular-nums">{formatTokens(segments.find((part)=>part.key===kind)?.tokens ?? 0,locale)}</span></span></li>)}
      <li className="tabular-nums">{t('usage.tokens.cacheHit',{percent:hit===null ? '–' : formatPercent(hit,locale)})}</li>
    </ul>
  </section>;
}
