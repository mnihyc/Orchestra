/** The small charts of the Overview: threads by state, turns and how long they took, machines connected, events waiting, what waits for a cap or a lock, and the blob store's room, over a day, a week, a month, a year or since the system was installed.
 *
 * One request brings every measurement for the span; each chart takes its lines from it. While the
 * lines of another span are on their way the charts hold what they showed, dimmed, so that nothing
 * jumps. What the server has not stored yet is said in the chart's own place.
 */
import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { metricsQuery } from '@/api/admin';
import {
  type Aggregate,
  CHART_MEASURES,
  type Group,
  groupSeries,
  linesOf,
  spanBounds,
  THREAD_STATES,
  TURN_RESULTS,
  WAITS,
} from '@/features/admin/charts';
import type { Span } from '@/features/admin/search';
import type { MetricSeries } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { boxClasses } from '@/shell/Page';
import { cx } from '@/ui/cx';
import { type ChartWords, TimeSeriesChart } from '@/ui/TimeSeriesChart';
import { ErrorState } from '@/ui/ErrorState';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

type Unit = 'count' | 'bytes' | 'seconds';

/** Says a key in the language of the reader, or the plain word where the catalogue has none: a state or a label this build does not know. */
type Translate = (key: string, fallback: string) => string;

type ChartId = 'threads' | 'turns' | 'turnTime' | 'machines' | 'events' | 'waiting' | 'room';

interface ChartDef {
  id: ChartId;
  unit: Unit;
  /** The lines it draws, from the series of the span. */
  groups: (series: readonly MetricSeries[]) => Group[];
  /** The places its lines take in the colours, by what they are. */
  order: readonly string[];
  /** How a line is named, for a value of the label it was grouped by. */
  name: (value: string, chart: ChartId, translate: Translate) => string;
}

const ROOM_ORDER = ['pinned', 'cache', 'free'] as const;

const by = (measure: string, label: string | null, how: Aggregate) => (series: readonly MetricSeries[]) => groupSeries(series, measure, label, how);

const CHARTS: readonly ChartDef[] = [
  { id: 'threads', unit: 'count', groups: by('threads', 'state', 'sum'), order: THREAD_STATES, name: (value, _chart, translate) => translate(`state.${value}`, value) },
  { id: 'turns', unit: 'count', groups: by('turns_total', 'status', 'sum'), order: TURN_RESULTS, name: (value, _chart, translate) => translate(`admin.overview.results.${value}`, value) },
  { id: 'turnTime', unit: 'seconds', groups: by('turn_seconds', null, 'mean'), order: [], name: (_value, chart, translate) => translate(`admin.overview.charts.${chart}`, chart) },
  { id: 'machines', unit: 'count', groups: by('link_sessions', null, 'sum'), order: [], name: (_value, chart, translate) => translate(`admin.overview.charts.${chart}`, chart) },
  { id: 'events', unit: 'count', groups: by('outbox_pending', null, 'sum'), order: [], name: (_value, chart, translate) => translate(`admin.overview.charts.${chart}`, chart) },
  { id: 'waiting', unit: 'count', groups: by('turns_waiting', 'reason', 'sum'), order: WAITS, name: (value, _chart, translate) => translate(`admin.overview.waits.${value}`, value) },
  {
    id: 'room',
    unit: 'bytes',
    groups: (series) => {
      const used = groupSeries(series, 'blob_store_bytes', 'class', 'sum');
      const free = groupSeries(series, 'blob_store_free_bytes', null, 'sum')[0];
      return free === undefined ? used : [...used, ['free', free[1]]];
    },
    order: ROOM_ORDER,
    name: (value, _chart, translate) => translate(`admin.overview.room.${value}`, value),
  },
];

function useFormat(unit: Unit): (value: number) => string {
  const locale = useAppearance((state) => state.locale);
  return useMemo(() => {
    if (unit === 'bytes') return (value: number) => formatBytes(value, locale);
    if (unit === 'seconds') {
      const seconds = new Intl.NumberFormat(locale, { style: 'unit', unit: 'second', unitDisplay: 'narrow', maximumFractionDigits: 1 });
      return (value: number) => seconds.format(value);
    }
    return (value: number) => formatNumber(value, locale, { maximumFractionDigits: 1 });
  }, [unit, locale]);
}

function ChartCard({ def, series, from, to, stale }: { def: ChartDef; series: readonly MetricSeries[]; from: number; to: number; stale: boolean }) {
  const { t, i18n } = useTranslation();
  const format = useFormat(def.unit);
  const lines = useMemo(() => {
    const translate: Translate = (key, fallback) => (i18n.exists(key) ? (t as (key: string) => string)(key) : fallback);
    return linesOf(def.groups(series), def.order, (value) => def.name(value, def.id, translate), t('admin.overview.other'));
  }, [def, series, t, i18n]);
  const words: ChartWords = {
    table: t('admin.overview.chartTable'),
    time: t('admin.overview.chartTime'),
    now: t('admin.overview.chartNow'),
    peak: t('admin.overview.chartPeak'),
    keys: t('admin.overview.chartKeys'),
  };
  const title = t(`admin.overview.charts.${def.id}`);
  return (
    <section aria-labelledby={`chart-${def.id}`} className={cx(boxClasses, 'flex min-w-0 flex-col gap-3 p-4')}>
      <h3 id={`chart-${def.id}`} className="text-base font-medium text-text">
        {title}
      </h3>
      <TimeSeriesChart label={title} lines={lines} format={format} from={from} to={to} words={words} stale={stale} empty={t('admin.overview.nothingStored')} />
    </section>
  );
}

/** The charts for one span. */
export function OverviewCharts({ span }: { span: Span }) {
  const { t } = useTranslation();
  // A historical chart is one question at the time it opens; a clock tick starts no HTTP read.
  const [minute] = useState(() => Math.floor(Date.now() / 60_000));
  const { from, to } = useMemo(() => spanBounds(span, minute * 60_000), [span, minute]);
  const request = useMemo(() => ({ names: CHART_MEASURES, from: new Date(from).toISOString(), to: new Date(to).toISOString() }), [from, to]);
  const metrics = useQuery({ ...metricsQuery(request), placeholderData: keepPreviousData });

  if (metrics.isPending) {
    return (
      <SkeletonGroup className="grid gap-4 md:grid-cols-2">
        {CHARTS.map((def) => (
          <div key={def.id} className={cx(boxClasses, 'flex flex-col gap-3 p-4')}>
            <Skeleton className="h-4 w-1/3" />
            <Skeleton className="h-40 w-full" />
          </div>
        ))}
      </SkeletonGroup>
    );
  }
  if (metrics.isError) {
    return <ErrorState error={metrics.error} title={t('admin.overview.chartsFailed')} retrying={metrics.isRefetching} onRetry={() => void metrics.refetch()} />;
  }
  return (
    <div className="grid gap-4 md:grid-cols-2">
      {CHARTS.map((def) => (
        <ChartCard key={def.id} def={def} series={metrics.data} from={from} to={to} stale={metrics.isPlaceholderData} />
      ))}
    </div>
  );
}
