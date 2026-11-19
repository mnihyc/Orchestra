/** The usage board of a place: turns, tokens and credits in a period, over time, by person and divided one other way, for whom its row names; Open usage.
 *
 * A card on the overview of an org, a project and a topic. It shows what was used there this week unless
 * the reader picks another period: the three figures, a small chart by day, who used how much with the
 * reader's own line among them or under them, and one line that divides the same total by model, by
 * provider, by agent or by kind of environment. Somebody the place does not let see its figures has
 * their own line alone, and "Your usage" leads on. No thread is on it. Where some use had no rate, the
 * credits say how many tokens that was.
 */
import { useListChoice } from '@/lib/listState';
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { usageQuery } from '@/api/usage';
import type { UsageAnalytics, UsageDimension, UsageFigures } from '@/generated/api-types';
import { creditsState, decimal, ESTIMATE_MARK, formatCredits, measureOf } from '@/features/usage/figures';
import { keyId, keyLabel } from '@/features/usage/keys';
import { spanOf, today, type UsageSearch } from '@/features/usage/search';
import { useFormatContext } from '@/lib/appearance';
import { DAY_MS } from '@/lib/time';
import { formatNumber, formatPercent, formatTokens } from '@/lib/format';
import { boxClasses, Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { Skeleton } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';
import { UsageVisibility } from './Visibility';

/** The periods the board offers. */
export const BOARD_PERIODS = ['day', 'week', 'last_week', 'month', 'year', 'all'] as const;
export type BoardPeriod = (typeof BOARD_PERIODS)[number];

/** What a board opens on: this week. */
const DEFAULT_PERIOD: BoardPeriod = 'week';

/** The other ways the board divides the same total. */
const OTHER_WAYS = ['model', 'provider', 'agent', 'environment'] as const satisfies readonly UsageDimension[];
type OtherWay = (typeof OTHER_WAYS)[number];

/** How many people the board lists before "Show all". */
const PEOPLE_SHOWN = 5;

/** How many rows it asks for: the ten who used most. */
const PEOPLE_ASKED = 10;

/** How many of the other way's members the line names before it says "other". */
const OTHER_NAMED = 3;

/** The days a period covers, as of today, in UTC. */
export function boardSpan(period: BoardPeriod, now: number): { from: string; to: string } {
  const to = today(now);
  const midnight = Date.parse(`${to}T00:00:00Z`);
  const date = new Date(midnight);
  switch (period) {
    case 'day':
      return { from: to, to };
    case 'week':
      return { from: new Date(midnight - ((date.getUTCDay() + 6) % 7) * DAY_MS).toISOString().slice(0, 10), to };
    case 'last_week': {
      const span=spanOf({range:'last_week'},now);
      return {from:span.from,to:span.to};
    }
    case 'month':
      return { from: `${to.slice(0, 8)}01`, to };
    case 'year':
      return { from: `${to.slice(0, 4)}-01-01`, to };
    case 'all':
      return { from: '2020-01-01', to };
  }
}

/** How many buckets a period has once it is over, so that the bars of one still going stand in their places and the days to come are empty. */
export function boardSlots(period: BoardPeriod, now: number): number | undefined {
  const date = new Date(now);
  switch (period) {
    case 'day':
      return 24;
    case 'week':
    case 'last_week':
      return 7;
    case 'month':
      return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0)).getUTCDate();
    case 'year':
      return 53;
    case 'all':
      return undefined;
  }
}

/** The search of the usage screen that shows what the board shows. */
function openSearch(nodeId: string, period: BoardPeriod, span: { from: string; to: string }): UsageSearch {
  const scope = `node:${nodeId}`;
  if (period === 'year' || period === 'all' || period === 'last_week') return { scope, range: period };
  return { scope, from: span.from, to: span.to };
}

/** The credits of a row, short: "151 cr", "≈ 4 cr" for an estimate, "–" for use that had no rate. */
export function creditsShort(figures: UsageFigures, locale: string, unit: string): string {
  const state = creditsState(figures);
  if (state.none) return '–';
  return `${state.allEstimated ? `${ESTIMATE_MARK} ` : ''}${formatCredits(state.sum, locale)} ${unit}`;
}

/** A place in a ranking in the words of the language: "5th" in English, "第 5 名" in Chinese. */
export function rankWords(place: number, locale: string, t: TFunction): string {
  const category = new Intl.PluralRules(locale, { type: 'ordinal' }).select(place);
  const key = category === 'one' || category === 'two' || category === 'few' ? category : 'other';
  return t(`usage.board.rank.${key}`, { n: formatNumber(place, locale) });
}

/** The small chart by day: one bar for each bucket, as tall as it is used, and nothing for a bucket that was not; `slots` keeps the buckets to come in their places. */
export function MiniBars({ analytics, label, slots }: { analytics: UsageAnalytics; label: string; slots?: number | undefined }) {
  const measure = decimal(analytics.totals.credits) > 0 ? 'credits' : 'tokens';
  const values = analytics.series.map((point) => measureOf(point.figures, measure));
  const largest = Math.max(...values, 0);
  if (largest === 0) return null;
  const later = Math.max(0, (slots ?? 0) - values.length);
  return (
    <div role="img" aria-label={label} className="flex h-8 items-end border-b border-border">
      {values.map((value, at) => (
        <span key={analytics.series[at]?.at ?? at} className="flex h-full min-w-px flex-1 items-end justify-center px-px">
          <span className="w-full max-w-[1.5rem] rounded-t-sm bg-accent" style={{ height: `${value === 0 ? 0 : Math.max(4, (value / largest) * 100)}%` }} />
        </span>
      ))}
      {Array.from({ length: later }, (_, at) => (
        <span key={`later-${at}`} aria-hidden className="min-w-px flex-1" />
      ))}
    </div>
  );
}

export interface UsageBoardProps {
  nodeId: string;
  name: string;
  className?: string;
}

export function UsageBoard({ nodeId, name, className }: UsageBoardProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [now] = useState(Date.now);
  const [period, setPeriod] = useListChoice<BoardPeriod>('usage-board', 'period', DEFAULT_PERIOD);
  const [other, setOther] = useListChoice<OtherWay>('usage-board', 'other', 'model');
  const span = boardSpan(period, now);
  const query = useQuery(usageQuery({ scope: `node:${nodeId}`, from: span.from, to: span.to, breakdowns: ['person', other], limit: PEOPLE_ASKED, filters: {} }));
  const analytics = query.data;
  const unit = t('usage.board.credits');
  const short = t('usage.board.creditsShort');
  const [open, setOpen] = useState(false);

  const people = analytics?.breakdowns.find((table) => table.by === 'person');
  const rest = analytics?.breakdowns.find((table) => table.by === other);
  const me = analytics?.me ?? null;
  const largestShare = Math.max(0, ...(people?.rows.map((row) => row.share) ?? []));
  const shown = people === undefined ? [] : open ? people.rows : people.rows.slice(0, PEOPLE_SHOWN);
  const userId = useStore(client.session, (state) => state.userId);
  const myRank = people === undefined ? -1 : people.rows.findIndex((row) => row.key.person?.id === userId);
  const own = analytics?.depth === 'own';

  const header = (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
      <div className="flex items-center gap-2">
        <Select
          size="sm"
          label={t('usage.board.period')}
          value={period}
          onValueChange={setPeriod}
          options={BOARD_PERIODS.map((one) => ({ value: one, name: t(`usage.board.periods.${one}.name`), description: t(`usage.board.periods.${one}.description`) }))}
          fieldClassName="w-auto"
        />
        <UsageVisibility nodeId={nodeId} />
        <Link to="/usage" search={openSearch(nodeId, period, span)} className="text-sm text-accent-text hover:underline">
          {t('usage.board.open')}
        </Link>
      </div>
      {analytics !== undefined && (
        <div className="text-sm text-text-muted tabular-nums">
          <ul className="flex flex-wrap gap-x-4 gap-y-1">
          {[
            t('usage.tables.turnsCount', { count: analytics.totals.turns }),
            t('usage.board.tokens', { tokens: formatTokens(analytics.totals.tokens.total_tokens, locale) }),
            creditsState(analytics.totals).none ? t('usage.board.noCredits') : `${formatCredits(decimal(analytics.totals.credits), locale)} ${unit}`,
          ].map((value,index)=><li key={index}>{value}</li>)}
          </ul>
          {analytics.totals.unrated_tokens > 0 && !creditsState(analytics.totals).none && <span> {t('usage.board.unrated', { tokens: formatTokens(analytics.totals.unrated_tokens, locale) })}</span>}
        </div>
      )}
    </div>
  );

  return (
    <div className={`${boxClasses} flex flex-col gap-3 p-4 ${className ?? ''}`}>
      {header}
      {query.isPending ? (
        <div className="flex flex-col gap-2">
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      ) : query.isError ? (
        <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
      ) : analytics === undefined ? null : analytics.totals.turns === 0 ? (
        <EmptyState compact title={t('usage.board.nothing', { name })} />
      ) : (
        <>
          <MiniBars analytics={analytics} label={t('usage.board.byBucket', { bucket: t(`usage.bucket.${analytics.bucket}`) })} slots={boardSlots(period, now)} />
          {own ? (
            <p className="text-sm text-text-muted">{t('usage.board.ownOnly', { name })}</p>
          ) : (
            <ol aria-label={t('usage.board.people')} className="flex flex-col gap-1">
              {shown.map((row, index) => (
                <li key={keyId('person', row.key)} className="grid grid-cols-[1.5rem_minmax(0,1fr)] items-center gap-x-2 gap-y-1 sm:grid-cols-[1.5rem_minmax(0,12rem)_minmax(0,1fr)_auto_auto_auto]">
                  <span className="text-sm text-text-subtle tabular-nums">{index + 1}</span>
                  {row.key.person === null || row.key.person === undefined ? <span className="text-text-muted">{keyLabel('person', row.key, t)}</span> : <UserChip user={row.key.person} size={24} />}
                  <span aria-hidden className="hidden h-2 overflow-hidden rounded-full bg-bg-muted sm:block">
                    <span className="block h-full rounded-full bg-accent" style={{ width: `${largestShare === 0 ? 0 : (row.share / largestShare) * 100}%` }} />
                  </span>
                  <span className="hidden text-right text-sm text-text tabular-nums sm:block">{formatTokens(row.figures.tokens.total_tokens, locale)}</span>
                  <span className="hidden text-right text-sm text-text-muted tabular-nums sm:block">{t('usage.tables.turnsCount', { count: row.figures.turns })}</span>
                  <span className="text-right text-sm text-text tabular-nums">{creditsShort(row.figures, locale, short)}</span>
                </li>
              ))}
            </ol>
          )}
          {!own && people !== undefined && (people.rows.length > PEOPLE_SHOWN || people.more > 0) && (
            <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-text-muted">
              {me !== null && myRank >= 0 ? <span>{t('usage.board.youAre', { rank: rankWords(myRank + 1, locale, t), tokens: formatTokens(me.tokens.total_tokens, locale), turns: t('usage.tables.turnsCount', { count: me.turns }), credits: creditsShort(me, locale, short) })}</span> : <span />}
              {!open && people.rows.length > PEOPLE_SHOWN ? (
                <Button size="sm" variant="ghost" onClick={() => setOpen(true)}>
                  {t('usage.board.showAll', { count: people.rows.length + people.more })}
                </Button>
              ) : people.more > 0 ? (
                <Link to="/usage" search={{ ...openSearch(nodeId, period, span), by: 'person' }} className="text-accent-text hover:underline">
                  {t('usage.board.showAll', { count: people.rows.length + people.more })}
                </Link>
              ) : null}
            </div>
          )}
          {rest !== undefined && rest.rows.length > 0 && (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border pt-3 text-sm">
              <Select
                size="sm"
                label={t('usage.board.divideBy')}
                value={other}
                onValueChange={setOther}
                options={OTHER_WAYS.map((one) => ({ value: one, name: t(`usagePerf.choices.splits.${one}.name`), description: t(`usagePerf.choices.splits.${one}.description`) }))}
                fieldClassName="w-auto"
              />
              <ul className="flex min-w-0 flex-1 flex-wrap gap-x-4 gap-y-1 text-text-muted">
                {[
                  ...rest.rows.slice(0, OTHER_NAMED).map((row) => `${keyLabel(other, row.key, t)} ${formatPercent(row.share, locale)}`),
                  ...(rest.rows.length > OTHER_NAMED || rest.more > 0
                    ? [`${t('usage.key.rest')} ${formatPercent(Math.max(0, 1 - rest.rows.slice(0, OTHER_NAMED).reduce((sum, row) => sum + row.share, 0)), locale)}`]
                    : []),
                ].map((value,index)=><li key={index}>{value}</li>)}
              </ul>
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** The board as a section of an overview, under the heading that says what it is. */
export function UsageSection({ nodeId, name, compact = false }: { nodeId: string; name: string; compact?: boolean }) {
  const { t } = useTranslation();
  return (
    <Section id={`usage-${nodeId}`} title={t('usage.board.section')}>
      {compact ? <WeeklyUsage nodeId={nodeId} /> : <UsageBoard nodeId={nodeId} name={name} />}
    </Section>
  );
}

/** An overview links its weekly totals to the full report without repeating that report's controls. */
function WeeklyUsage({ nodeId }: { nodeId: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [now] = useState(Date.now);
  const span = boardSpan('week', now);
  const query = useQuery(usageQuery({ scope: `node:${nodeId}`, ...span, breakdowns: [], filters: {} }));
  if (query.isPending) return <Skeleton className="h-5 w-2/3" />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const figures = query.data.totals;
  return <Link to="/usage" search={openSearch(nodeId, 'week', span)} className="focus-inset flex w-fit max-w-full flex-wrap items-baseline gap-x-3 gap-y-1 rounded-sm text-sm text-text-muted hover:text-accent-text" aria-label={t('usage.board.open')}>
    <span className="text-accent-text underline underline-offset-2">{t('usage.board.periods.week.name')}</span>
    {query.data.depth === 'own' && <span>{t('usage.title.me')}</span>}
    <span className="tabular-nums">{t('usage.tables.turnsCount', { count: figures.turns })}</span>
    <span className="tabular-nums">{t('usage.board.tokens', { tokens: formatTokens(figures.tokens.total_tokens, locale) })}</span>
    <span className="tabular-nums">{creditsState(figures).none ? t('usage.board.noCredits') : creditsShort(figures, locale, t('usage.board.credits'))}</span>
  </Link>;
}
