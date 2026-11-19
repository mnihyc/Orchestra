/** What a thread's turns used over time: each turn's tokens by part as a column, the context's fill as a line with its shortenings marked, the cache hit rate, the sub-agents' share, the same as a table, and an export.
 *
 * It is a section of the thread's Usage view, read turn by turn from what each turn kept of its harness's
 * report. Each column is reached by the keyboard and says its numbers; the table under the chart says them
 * all. Where the harness reports nothing, the section says so, by the harness's name, and draws nothing.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { modelDisplayName } from '@/features/models/names';
import { programNameIn } from '@/features/resources/programNames';

import { threadTurnUsageQuery } from '@/api/context';
import { turnsRevision } from '@/features/usage/context/ContextPanel';
import {
  cacheHitRate,
  COST_LOOKS,
  type CostPart,
  costParts,
  exportText,
  fillPercent,
  lookStyle,
} from '@/features/usage/context/fill';
import { Swatch } from '@/features/usage/context/PartsBar';
import { saveFile } from '@/features/usage/saveFile';
import type { Thread, TurnUsageRow } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatPercent, formatTokens } from '@/lib/format';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { EmptyState } from '@/ui/EmptyState';
import { SkeletonRows } from '@/ui/Skeleton';

const PARTS: readonly CostPart[] = ['fresh', 'cache_read', 'cache_write', 'output', 'reasoning', 'subagents'];

/** The sign of a turn in which the conversation was shortened, and of the line of the context's fill: signs beside words, no words themselves. */
const SHORTENED_MARK = '▼';
const FILL_MARK = '┄';

function Column({ row, largest }: { row: TurnUsageRow; largest: number }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const usage = row.usage ?? null;
  const parts = usage === null ? [] : costParts(usage, row.detail ?? null);
  const total = parts.reduce((sum, part) => sum + part.tokens, 0);
  const fill = fillPercent(row);
  const shortened = (row.detail?.shortenings.length ?? 0) > 0;
  const words = [
    t('contextUsage.time.turn', { ordinal: row.ordinal, tokens: formatTokens(usage?.total_tokens ?? 0, locale) }),
    ...parts.map((part) => t(`contextUsage.cost.${part.part}`, { tokens: formatTokens(part.tokens, locale) })),
    ...(fill === null ? [] : [t('contextUsage.time.fill', { percent: formatPercent(fill / 100, locale) })]),
    ...(shortened ? [t('contextUsage.time.shortened')] : []),
  ];
  const description = new Intl.ListFormat(locale).format(words);
  return (
    <div role="img" tabIndex={0} aria-label={description} title={description} className="relative flex h-full min-w-1 flex-1 flex-col-reverse outline-offset-1 focus-visible:outline-2 focus-visible:outline-accent" data-usage-turn={row.ordinal}>
      {parts.map((part) => (
        <span key={part.part} className="block w-full" style={{ height: `${largest > 0 ? (part.tokens / largest) * 100 : 0}%`, ...lookStyle(COST_LOOKS[part.part]) }} />
      ))}
      {total === 0 && <span className="block h-px w-full bg-border" />}
      {shortened && <span aria-hidden className="absolute -top-3 left-1/2 -translate-x-1/2 text-[0.6rem] leading-none text-text-muted" data-usage-shortened>{SHORTENED_MARK}</span>}
    </div>
  );
}

/** The line of the context's fill across the columns, in percent of the chart's height. */
function FillLine({ rows }: { rows: readonly TurnUsageRow[] }) {
  const points = rows
    .map((row, index) => ({ x: ((index + 0.5) / rows.length) * 100, fill: fillPercent(row) }))
    .filter((point): point is { x: number; fill: number } => point.fill !== null)
    .map((point) => `${point.x.toFixed(2)},${(100 - point.fill).toFixed(2)}`);
  if (points.length === 0) return null;
  return (
    <svg aria-hidden viewBox="0 0 100 100" preserveAspectRatio="none" className="pointer-events-none absolute inset-0 h-full w-full" data-usage-fill-line>
      <polyline points={points.join(' ')} fill="none" stroke="var(--text)" strokeWidth="1.5" strokeDasharray="3 2" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export function ThreadComposition({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const revision = useThreadPart(thread.id, turnsRevision);
  const query = useInfiniteQuery(threadTurnUsageQuery(thread.id, revision));
  const pages = query.data?.pages ?? [];
  const first = pages[0];
  const rows = [...pages].reverse().flatMap((page) => page.turns);
  const harness = programNameIn(first, locale) ?? programNameIn(thread.binding?.agent, locale) ?? t('contextUsage.harness.other');
  const reported = rows.filter((row) => row.usage !== null);
  const largest = Math.max(0, ...reported.map((row) => (row.usage === null ? 0 : costParts(row.usage, row.detail ?? null).reduce((sum, part) => sum + part.tokens, 0))));
  const input = reported.reduce((sum, row) => sum + (row.usage?.input_tokens ?? 0), 0);
  const cached = reported.reduce((sum, row) => sum + (row.usage?.cached_input_tokens ?? 0), 0);
  const total = reported.reduce((sum, row) => sum + (row.usage?.total_tokens ?? 0), 0);
  const subagents = reported.reduce((sum, row) => sum + (row.detail?.subagents?.total_tokens ?? 0), 0);
  return (
    <section className="flex flex-col gap-3 rounded-lg border border-border bg-surface p-4" aria-label={t('contextUsage.time.title')} data-thread-composition>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-base font-semibold text-text">{t('contextUsage.time.title')}</h2>
        {reported.length > 0 && (
          <Button size="sm" action="export" onClick={() => saveFile(exportText(rows), `orch-thread-${thread.id}-turns.csv`)}>
            {t('contextUsage.time.export')}
          </Button>
        )}
      </div>
      {query.isError ? (
        <ErrorAlert error={query.error} />
      ) : first === undefined ? (
        <SkeletonRows rows={3} />
      ) : !first.reports.tokens ? (
        <div data-usage-absent><EmptyState compact title={t('contextUsage.absent.noTokens', { harness })} /></div>
      ) : reported.length === 0 ? (
        <div data-usage-absent><EmptyState compact title={t('contextUsage.absent.noTurns')} /></div>
      ) : (
        <>
          <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted">
            {[
              input > 0 ? t('contextUsage.time.hitRate', { percent: formatPercent(cached / input, locale) }) : null,
              total > 0 ? t('contextUsage.time.subagents', { percent: formatPercent(subagents / total, locale) }) : null,
            ]
              .filter((words): words is string => words !== null)
              .map((words)=><span key={words}>{words}</span>)}
          </p>
          <div className="relative mt-3 flex h-[8rem] items-end gap-px border-b border-border" data-usage-chart>
            {rows.map((row) => <Column key={row.turn_id} row={row} largest={largest} />)}
            {first.reports.context && <FillLine rows={rows} />}
          </div>
          <ul className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted" aria-label={t('contextUsage.time.legend')}>
            {PARTS.map((part) => (
              <li key={part} className="inline-flex items-center gap-1">
                <Swatch segment={{ look: COST_LOOKS[part] }} />
                {t(`contextUsage.cost.name.${part}`)}
              </li>
            ))}
            {first.reports.context && <li className="inline-flex items-center gap-1"><span aria-hidden>{FILL_MARK}</span> {t('contextUsage.time.fillLegend')}</li>}
            <li className="inline-flex items-center gap-1"><span aria-hidden>{SHORTENED_MARK}</span> {t('contextUsage.time.shortenedLegend')}</li>
          </ul>
          {!first.reports.context && <p className="text-xs text-text-subtle">{t('contextUsage.absent.noContext', { harness })}</p>}
          <div className="max-h-[16rem] overflow-auto">
            <table className="w-full text-xs tabular-nums">
              <caption className="sr-only">{t('contextUsage.time.table')}</caption>
              <thead className="text-text-subtle">
                <tr>
                  <th scope="col" className="text-left font-medium">{t('contextUsage.time.columns.turn')}</th>
                  <th scope="col" className="text-left font-medium">{t('contextUsage.time.columns.model')}</th>
                  <th scope="col" className="text-right font-medium">{t('contextUsage.time.columns.total')}</th>
                  {PARTS.map((part) => <th key={part} scope="col" className="text-right font-medium">{t(`contextUsage.cost.name.${part}`)}</th>)}
                  <th scope="col" className="text-right font-medium">{t('contextUsage.time.columns.hit')}</th>
                  <th scope="col" className="text-right font-medium">{t('contextUsage.time.columns.fill')}</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const usage = row.usage ?? null;
                  const parts = new Map((usage === null ? [] : costParts(usage, row.detail ?? null)).map((part) => [part.part, part.tokens] as const));
                  const hit = usage === null ? null : cacheHitRate(usage);
                  const fill = fillPercent(row);
                  return (
                    <tr key={row.turn_id} className="border-t border-border" data-usage-row={row.ordinal}>
                      <td className="py-1">
                        {row.ordinal}
                        {(row.detail?.shortenings.length ?? 0) > 0 && <span aria-hidden className="text-text-subtle"> {SHORTENED_MARK}</span>}
                      </td>
                      <td className="py-1 pr-2">{modelDisplayName(row.model_display,t)}</td>
                      <td className="py-1 text-right">{usage === null ? '–' : formatTokens(usage.total_tokens, locale)}</td>
                      {PARTS.map((part) => <td key={part} className="py-1 text-right">{formatTokens(parts.get(part) ?? 0, locale)}</td>)}
                      <td className="py-1 text-right">{hit === null ? '–' : formatPercent(hit, locale)}</td>
                      <td className="py-1 text-right">{fill === null ? '–' : formatPercent(fill / 100, locale)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          {query.hasNextPage && (
            <Button size="sm" variant="ghost" pending={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>
              {t('contextUsage.time.older')}
            </Button>
          )}
        </>
      )}
    </section>
  );
}
