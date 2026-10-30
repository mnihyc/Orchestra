/** The same total divided by person, model, provider, agent, kind of environment and project, one table each; a row filters the whole screen.
 *
 * Each table is the total of the span divided by one dimension, the largest first, with its share, its
 * turns, its tokens and its credits. Pressing a row narrows the whole screen to that member, as a chip
 * above the chart; a member the reader may not see is a line without a name and is counted all the same.
 * A table of a dimension that has no filter (the speed, the org) is read and not pressed.
 */
import { useTranslation } from 'react-i18next';

import type { UsageBreakdown, UsageDimension, UsageRow } from '@/generated/api-types';
import { SumCredits } from '@/features/usage/Amounts';
import { creditsState, formatCredits } from '@/features/usage/figures';
import { filterValue, keyDetail, keyId, keyLabel } from '@/features/usage/keys';
import { filterOf, type FilterName, type UsageSearch } from '@/features/usage/search';
import { errorsText, speedText, timeText } from '@/features/usage/performance';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber, formatPercent, formatTokens } from '@/lib/format';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { Tabs } from '@/ui/Tabs';
import { UserChip } from '@/ui/UserChip';
import type { Split } from './search';
import { Section } from '@/shell/Page';

const PERFORMANCE_COLUMNS = { requests: 'requests', speed: 'speed', first: 'first', errors: 'errors' } as const;

/** What a table is while it is read: asked for, there, or not to be had. */
export type TableState = { state: 'loading' } | { state: 'failed'; error: unknown; retry: () => void } | { state: 'ready'; table: UsageBreakdown };

export interface BreakdownsProps {
  seriesBy?: Split | undefined;
  colors?: ReadonlyMap<string,number> | undefined;
  tabs: readonly UsageDimension[];
  tab: UsageDimension;
  onTab: (dimension: UsageDimension) => void;
  tableOf: (dimension: UsageDimension) => TableState;
  search: UsageSearch;
  onFilter: (name: FilterName, value: string) => void;
  /** The tables list a hundred rows. */
  showingMore: boolean;
  onMore: () => void;
  className?: string;
  performance?: boolean;
}

export function ShareBar({ share, slot }: { share: number; slot?:number | undefined }) {
  const { locale } = useFormatContext();
  const held = Math.min(1, Math.max(0, share));
  return (
    <span className="flex items-center gap-2">
      <span aria-hidden className="h-2 w-[6rem] shrink-0 overflow-hidden rounded-full bg-bg-muted">
        <span className="block h-full rounded-full" style={{ width: `${held * 100}%`,background:slot===undefined || slot===0 ? 'var(--text-subtle)' : `var(--chart-${slot})` }} />
      </span>
      <span className="w-10 text-right text-text-muted tabular-nums">{formatPercent(held, locale)}</span>
    </span>
  );
}

/** One member of a dimension as a row names it: a person with their face, anything else by its name and what else it is called. */
function Member({ dimension, row }: { dimension: UsageDimension; row: UsageRow }) {
  const { t } = useTranslation();
  const label = keyLabel(dimension, row.key, t);
  const detail = keyDetail(dimension, row.key, t);
  if (dimension === 'person' && row.key.person !== null && row.key.person !== undefined) {
    return <UserChip user={row.key.person} size={24} />;
  }
  return (
    <span className="flex min-w-0 flex-col">
      <span className="font-medium wrap-anywhere text-text">{label}</span>
      {detail !== null && <span className="text-sm wrap-anywhere text-text-subtle">{detail}</span>}
    </span>
  );
}

function RowsOf({ dimension, table, search, onFilter, showingMore, onMore, performance = false,seriesBy,colors }: { dimension: UsageDimension; table: UsageBreakdown; search: UsageSearch } & Pick<BreakdownsProps, 'onFilter' | 'showingMore' | 'onMore' | 'performance' | 'seriesBy' | 'colors'>) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const name = filterOf(dimension);
  const active = name === null ? undefined : search[name];
  const NARROW_COLUMN = 'credits';

  const columns: Column<UsageRow>[] = [
    {
      id: 'name',
      header: t(`usage.tables.by.${dimension}`),
      cell: (row, shown) => {
        const value = name === null ? null : filterValue(dimension, row.key);
        const pressed = value !== null && active === value;
        const content = <Member dimension={dimension} row={row} />;
        const p = row.figures.performance;
        const compact = !performance ? [] : [
          !shown.has(PERFORMANCE_COLUMNS.requests) ? t('usagePerf.compactRequests', { count: p?.requests ?? 0, value: formatNumber(p?.requests ?? 0, locale) }) : null,
          !shown.has(PERFORMANCE_COLUMNS.speed) ? t('usagePerf.compactSpeed',{value:speedText(p?.speed_milli === null || p?.speed_milli === undefined ? null : p.speed_milli / 1000, locale, t)}) : null,
          !shown.has(PERFORMANCE_COLUMNS.first) ? t('usagePerf.compactFirst', { value: timeText(p?.first_token_ms, locale, t) }) : null,
          !shown.has(PERFORMANCE_COLUMNS.errors) ? t('usagePerf.compactErrors', { value: errorsText(p, locale, t) }) : null,
        ].filter((part):part is string=>part!==null);
        return (
          <span className="flex min-w-0 flex-col gap-1">
            {name === null || value === null ? (
              content
            ) : (
              <button
                type="button"
                aria-pressed={pressed}
                title={t('usage.tables.narrow', { name: keyLabel(dimension, row.key, t) })}
                onClick={() => onFilter(name, value)}
                className={cx('flex min-w-0 rounded-md text-left hover:underline', pressed && 'underline')}
              >
                {content}
              </button>
            )}
            {!performance && !shown.has(NARROW_COLUMN) && (
              <span className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
                {[
                  t('usage.tables.turnsCount', { count: row.figures.turns }),
                  t('usage.board.tokens',{tokens:formatTokens(row.figures.tokens.total_tokens, locale)}),
                  `${t('usage.tables.credits')}: ${creditsState(row.figures).none ? '–' : formatCredits(creditsState(row.figures).sum, locale)}`,
                ].map((part,index)=><span key={index}>{part}</span>)}
              </span>
            )}
            {compact.map((part)=><span key={part} className="text-sm text-text-muted">{part}</span>)}
          </span>
        );
      },
    },
    { id: 'share', header: t('usage.tables.share'), from: 'md', className: 'w-[11rem]', cell: (row) => <ShareBar share={row.share} slot={seriesBy===dimension ? colors?.get(keyId(dimension,row.key)) : undefined} /> },
    { id: 'turns', header: t('usage.tables.turns'), from: 'sm', align: 'end', cell: (row) => <span className="tabular-nums">{formatNumber(row.figures.turns, locale)}</span> },
    { id: 'tokens', header: t('usage.tables.tokens'), from: 'sm', align: 'end', cell: (row) => <span className="tabular-nums">{formatTokens(row.figures.tokens.total_tokens, locale)}</span> },
    { id: 'credits', header: t('usage.tables.credits'), from: 'sm', align: 'end', cell: (row) => <SumCredits figures={row.figures} /> },
  ];
  if (performance) {
    columns.splice(1);
    columns.push(
      { id: PERFORMANCE_COLUMNS.requests, header: t('usagePerf.requests'), from: 'sm', align: 'end', cell: (row) => formatNumber(row.figures.performance?.requests ?? 0, locale) },
      { id: PERFORMANCE_COLUMNS.speed, header: t('usagePerf.speed'), from: 'md', align: 'end', cell: (row) => speedText(row.figures.performance?.speed_milli === null || row.figures.performance?.speed_milli === undefined ? null : row.figures.performance.speed_milli / 1000, locale, t) },
      { id: PERFORMANCE_COLUMNS.first, header: t('usagePerf.firstToken'), from: 'md', align: 'end', cell: (row) => timeText(row.figures.performance?.first_token_ms, locale, t) },
      { id: PERFORMANCE_COLUMNS.errors, header: t('usagePerf.errors'), from: 'lg', align: 'end', cell: (row) => errorsText(row.figures.performance,locale,t) },
    );
  }

  if (table.rows.length === 0) {
    return <p className="text-sm text-text-muted">{t('usage.tables.empty')}</p>;
  }
  return (
    <div className="flex flex-col gap-2">
      <Table label={t(`usage.tables.by.${dimension}`)} columns={columns} rows={table.rows} rowKey={(row) => keyId(dimension, row.key)} />
      {table.more > 0 && (
        <p className="flex flex-wrap items-center gap-x-3 text-sm text-text-muted">
          <span>{t('usage.tables.more', { count: table.more })}</span>
          {!showingMore && (
            <Button size="sm" variant="ghost" onClick={onMore}>
              {t('usage.tables.showMore')}
            </Button>
          )}
        </p>
      )}
    </div>
  );
}

export function Breakdowns({ tabs, tab, onTab, tableOf, search, onFilter, showingMore, onMore, className, performance = false,seriesBy,colors }: BreakdownsProps) {
  const { t } = useTranslation();
  const shown = tabs.includes(tab) ? tab : (tabs[0] ?? 'model');
  return (
    <Tabs
      frame={(navigation,panels)=><Section id="usage-breakdown" title={t('usage.sections.breakdown')} action={navigation}>{panels}</Section>}
      className={className}
      label={t('usage.tables.label')}
      value={shown}
      onValueChange={onTab}
      tabs={tabs.map((dimension) => ({
        value: dimension,
        label: dimension === 'thread' ? t('usage.tables.by.thread') : t(`usagePerf.choices.splits.${dimension}.name`),
        menuLabel: t(`usage.tables.by.${dimension}`),
        panel: <div className="flex min-w-0 flex-col gap-3">{(() => {
          const state = tableOf(dimension);
          if (state.state === 'loading') return <SkeletonRows rows={4} />;
          if (state.state === 'failed') return <ErrorState compact error={state.error} onRetry={state.retry} />;
          return <RowsOf dimension={dimension} table={state.table} search={search} onFilter={onFilter} showingMore={showingMore} onMore={onMore} performance={performance} seriesBy={seriesBy} colors={colors} />;
        })()}</div>,
      }))}
    />
  );
}
