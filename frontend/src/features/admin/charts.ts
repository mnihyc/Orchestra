/** What the charts of the Overview are made of: the span each one covers, which measurements it asks for, and how what the server stored becomes lines.
 *
 * The server stores, for every measurement it keeps, a value for each minute for a while and a value
 * for each hour for good; a request names the measurements and a span and gets back at most so many
 * points for each. Here are the spans a reader chooses among, the grouping of the series by one label
 * (threads by state), and the colour each line takes by what it is of, never by its rank.
 */
import type { MetricSeries } from '@/generated/api-types';
import type { Span } from '@/features/admin/search';
import type { ChartLine } from '@/ui/timeSeries';

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

/**
 * How far back each span reaches. A month is 719 hours, and not thirty days: a request is answered at
 * hourly steps only when the span touches at most 720 of them, and that is one hour short of thirty
 * days. The longest span, "since installed", is as many days as one request is answered for at the
 * step of a day; the chart begins where the first stored value is.
 */
export const SPAN_MS: Record<Span, number> = {
  day: DAY_MS,
  week: 7 * DAY_MS,
  month: 719 * HOUR_MS,
  year: 365 * DAY_MS,
  all: 719 * DAY_MS,
};

/** The moments a span covers when it ends at `now`, rounded down to the minute so that the same minute asks the same thing. */
export function spanBounds(span: Span, now: number): { from: number; to: number } {
  const to = Math.floor(now / MINUTE_MS) * MINUTE_MS;
  return { from: to - SPAN_MS[span], to };
}

/** The measurements the Overview draws, by the name the server stores them under. */
export const CHART_MEASURES = [
  'threads',
  'turns_total',
  'turn_seconds',
  'link_sessions',
  'outbox_pending',
  'turns_waiting',
  'blob_store_bytes',
  'blob_store_free_bytes',
] as const;

export type Aggregate = 'sum' | 'mean';

/** One group of points: what the series that share a label value come to at each moment. */
export type Group = [value: string, points: [number, number][]];

/**
 * The series named `name` laid together by the value of one label, in the order of the first
 * appearance of each value. At a moment where several series of one group have a point they are added
 * (a count is the sum of its parts), or averaged where the measurement is a duration.
 */
export function groupSeries(series: readonly MetricSeries[], name: string, by: string | null, how: Aggregate): Group[] {
  const groups = new Map<string, Map<number, { sum: number; count: number }>>();
  for (const one of series) {
    if (one.name !== name) continue;
    const value = by === null ? '' : (one.labels[by] ?? '');
    let moments = groups.get(value);
    if (moments === undefined) {
      moments = new Map();
      groups.set(value, moments);
    }
    for (const [at, amount] of one.points) {
      const stamp = Date.parse(at);
      if (Number.isNaN(stamp)) continue;
      const seen = moments.get(stamp) ?? { sum: 0, count: 0 };
      seen.sum += amount;
      seen.count += 1;
      moments.set(stamp, seen);
    }
  }
  return [...groups].map(([value, moments]): Group => [
    value,
    [...moments]
      .sort(([a], [b]) => a - b)
      .map(([stamp, seen]): [number, number] => [stamp, how === 'mean' ? seen.sum / seen.count : seen.sum]),
  ]);
}

/** The most lines a chart has: one for each colour it has, the last of which may be "everything else". */
export const MOST_LINES = 8;

/**
 * Gives each group a colour by what it is. Those named in `order` have their own place in it and keep
 * it whichever of them are there; others take the colours after those, by name. A chart with more
 * groups than colours keeps the first and adds the rest together under the last colour.
 */
export function linesOf(groups: readonly Group[], order: readonly string[], name: (value: string) => string, other: string): ChartLine[] {
  const known = groups.filter(([value]) => order.includes(value)).sort(([a], [b]) => order.indexOf(a) - order.indexOf(b));
  const unknown = groups.filter(([value]) => !order.includes(value)).sort(([a], [b]) => a.localeCompare(b));
  // The colours that `order` does not give out.
  const spare = Math.max(0, MOST_LINES - order.length);
  const folded = unknown.length > spare;
  const kept = folded ? unknown.slice(0, Math.max(0, spare - 1)) : unknown;
  const rest = folded ? unknown.slice(kept.length) : [];

  const lines: ChartLine[] = known.map(([value, points]) => ({ id: value, label: name(value), slot: order.indexOf(value) + 1, points }));
  kept.forEach(([value, points], at) => {
    lines.push({ id: value === '' ? 'all' : value, label: name(value), slot: order.length + 1 + at, points });
  });
  if (rest.length > 0) {
    const together = new Map<number, number>();
    for (const [, points] of rest) for (const [at, amount] of points) together.set(at, (together.get(at) ?? 0) + amount);
    lines.push({ id: 'other', label: other, slot: MOST_LINES, points: [...together].sort(([a], [b]) => a - b) });
  }
  return lines;
}

/** The states a thread is in, in the order they take the chart's colours. */
export const THREAD_STATES = ['running', 'waiting_approval', 'blocked', 'failed', 'idle', 'done'] as const;
/** The results of a turn. */
export const TURN_RESULTS = ['completed', 'failed', 'interrupted'] as const;
/** What a turn waits for. */
export const WAITS = ['cap', 'lock'] as const;
