/** Shared TimeSeriesChart component (measures); primitives are wrapped here.
 *
 * A few lines over a span of time, drawn by uPlot on a canvas that is loaded when the first chart is
 * shown and by nothing else. Every line is two pixels thick and keeps its colour for what it is of;
 * a crosshair follows the pointer (and the arrow keys) and one readout lists every line at that
 * moment; the legend under the chart carries what each line is now and the most it has been, and the
 * values are also in a table that stands under the chart, so that nothing is told by colour or by the
 * pointer alone. The words are given by the caller, so that the control knows no catalogue.
 */
import type uPlot from 'uplot';
import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react';

import { useAppearance } from '@/lib/appearance';
import { cx } from '@/ui/cx';
import { Preview, PREVIEW_LINES } from '@/ui/Preview';
import {
  type Aligned,
  alignLines,
  type ChartLine,
  figuresOf,
  largestOf,
  nearestIndex,
  sampleRows,
  scaleTop,
  withAlpha,
} from '@/ui/timeSeries';

export type { ChartLine } from '@/ui/timeSeries';

/** The words a chart says, in the language of its reader. */
export interface ChartWords {
  /** The name of the table that says the same as the chart. */
  table: string;
  /** The head of the column of moments in that table. */
  time: string;
  now: string;
  peak: string;
  /** How a reader finds the readout without a pointer. */
  keys: string;
}

export interface TimeSeriesChartProps {
  /** What is plotted: it names the chart for a screen reader and heads the table. */
  label: string;
  lines: readonly ChartLine[];
  /** Writes a value for the scale, the readout, the legend and the table. */
  format: (value: number) => string;
  /** The span the time scale covers, in milliseconds: the chart does not shrink to its first point. */
  from: number;
  to: number;
  words: ChartWords;
  /** What stands in the chart's place while no line has a point. */
  empty: ReactNode;
  height?: number;
  /** The span of another request is on its way: the render that is there is held, dimmed. */
  stale?: boolean;
  className?: string;
}

const DEFAULT_HEIGHT = 160;

/** The wash under a line, as a share of its colour: a wash and never a block. */
const WASH = 0.1;

/** The height of the band of the time scale, and the fewest pixels between two of its labels. */
const TIME_AXIS_SIZE = 26;
const TIME_TICK_SPACE = 100;

/** Room round the widest figure of the value axis, in pixels. */
const AXIS_PADDING = 16;

/** Room to the right of the plot for half of the label of a tick at the very end of the span, which would be cut off at the edge. */
const END_ROOM = 28;

/** Where the readout stands relative to the crosshair, in pixels. */
const READOUT_GAP = 12;

/** The chart colours a theme gives, by slot from 1. */
const SLOTS = 8;

interface Colors {
  series: string[];
  grid: string;
  axis: string;
  ink: string;
  surface: string;
  font: string;
}

function readColors(element: HTMLElement): Colors {
  const css = getComputedStyle(element);
  const read = (name: string) => css.getPropertyValue(name).trim();
  return {
    series: Array.from({ length: SLOTS }, (_, slot) => read(`--chart-${slot + 1}`)),
    grid: read('--border'),
    axis: read('--border-strong'),
    ink: read('--text-muted'),
    surface: read('--surface'),
    font: `12px ${css.fontFamily}`,
  };
}

/** The colour of a slot, counted from 1. */
const colorOf = (slot: number, colors: readonly string[]): string => colors[(slot - 1) % SLOTS] ?? '';

/** The class that colours the key of a slot: written out, so that the utility generator finds each. */
const KEY_CLASSES = ['bg-chart-1', 'bg-chart-2', 'bg-chart-3', 'bg-chart-4', 'bg-chart-5', 'bg-chart-6', 'bg-chart-7', 'bg-chart-8'] as const;

interface Hover {
  index: number;
  /** Where the crosshair is, in pixels from the left of the plot area. */
  left: number;
  fromKeyboard: boolean;
}

/** A moment as a date in the time zone of the reader; their own zone where the name of it is not one this browser knows. */
function inZone(make: (date: Date, timeZone: string) => Date, stamp: number, timeZone: string): Date {
  const date = new Date(stamp * 1000);
  try {
    return make(date, timeZone);
  } catch {
    return date;
  }
}

/** Ticks of the time scale in the language and the time zone of the reader; the span decides how much each says. */
function timeTicks(locale: string, timeZone: string | undefined, spanMs: number): (stamp: number) => string {
  const day = spanMs <= 2 * 24 * 3_600_000;
  const options: Intl.DateTimeFormatOptions = day ? { hour: 'numeric', minute: '2-digit' } : { month: 'short', day: 'numeric' };
  let format: Intl.DateTimeFormat;
  try {
    format = new Intl.DateTimeFormat(locale, { ...options, ...(timeZone === undefined ? {} : { timeZone }) });
  } catch {
    format = new Intl.DateTimeFormat(locale, options);
  }
  return (stamp) => format.format(new Date(stamp * 1000));
}

function readoutTime(locale: string, timeZone: string | undefined): (ms: number) => string {
  const options: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' };
  let format: Intl.DateTimeFormat;
  try {
    format = new Intl.DateTimeFormat(locale, { ...options, ...(timeZone === undefined ? {} : { timeZone }) });
  } catch {
    format = new Intl.DateTimeFormat(locale, options);
  }
  return (ms) => format.format(new Date(ms));
}

export function TimeSeriesChart({ label, lines, format, from, to, words, empty, height = DEFAULT_HEIGHT, stale = false, className }: TimeSeriesChartProps) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);
  const theme = useAppearance((state) => state.resolvedTheme);
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const [hover, setHover] = useState<Hover | null>(null);
  const [width, setWidth] = useState(0);

  const aligned: Aligned = useMemo(() => alignLines(lines), [lines]);
  const hasPoints = aligned.xs.length > 0;

  // How wide the room is; the chart is made once there is some, and resized after that.
  useEffect(() => {
    const element = host.current;
    if (element === null) return;
    setWidth(element.clientWidth);
    const watch = new ResizeObserver(() => setWidth(element.clientWidth));
    watch.observe(element);
    return () => watch.disconnect();
  }, []);

  const ready = width > 0 && hasPoints;
  useEffect(() => {
    const element = host.current;
    if (element === null || !ready) return;
    let cancelled = false;
    void Promise.all([import('uplot'), import('uplot/dist/uPlot.min.css')]).then(([module]) => {
      if (cancelled) return;
      const UPlot = module.default;
      const colors = readColors(element);
      const tick = timeTicks(locale, timeZone, to - from);
      let widestSeen = AXIS_PADDING + 24;
      const options: uPlot.Options = {
        width: element.clientWidth,
        height,
        legend: { show: false },
        select: { show: false, left: 0, top: 0, width: 0, height: 0 },
        padding: [8, END_ROOM, 0, 0],
        tzDate: (stamp) => inZone((date, zone) => UPlot.tzDate(date, zone), stamp, timeZone),
        cursor: {
          x: true,
          y: false,
          drag: { x: false, y: false, setScale: false },
          // A dot with a ring of the surface round it, so that it reads where lines cross.
          points: { size: 8, width: 2, stroke: colors.surface, fill: (_chart, index) => colorOf(lines[index - 1]?.slot ?? 1, colors.series) },
        },
        scales: {
          x: { time: true, range: [from / 1000, to / 1000] },
          y: { range: (_chart, _min, max) => [0, scaleTop(max)] },
        },
        axes: [
          {
            stroke: colors.ink,
            font: colors.font,
            // Room for a label of the time scale and no more, and ticks far enough apart that two labels never touch.
            size: TIME_AXIS_SIZE,
            gap: 4,
            space: TIME_TICK_SPACE,
            grid: { stroke: colors.grid, width: 1 },
            ticks: { stroke: colors.grid, width: 1, size: 4 },
            border: { stroke: colors.axis, width: 1 },
            values: (_chart, splits) => splits.map((stamp) => tick(stamp)),
          },
          {
            stroke: colors.ink,
            font: colors.font,
            grid: { stroke: colors.grid, width: 1 },
            ticks: { show: false },
            border: { show: false },
            values: (_chart, splits) => splits.map((value) => format(value)),
            // As wide as the widest figure it writes.
            size: (self, values, _axisIndex, cycle) => {
              // The first answer stands once the layout has been tried again: it is what the labels need.
              if (cycle > 1) return widestSeen;
              let widest = 0;
              self.ctx.font = colors.font;
              for (const value of (values as string[] | null) ?? []) widest = Math.max(widest, self.ctx.measureText(value).width / devicePixelRatio);
              widestSeen = Math.ceil(widest) + AXIS_PADDING;
              return widestSeen;
            },
          },
        ],
        series: [
          {},
          ...lines.map((line): uPlot.Series => {
            const stroke = colorOf(line.slot, colors.series);
            return {
              label: line.label,
              stroke,
              width: 2,
              spanGaps: true,
              // A lone point is a dot: a line needs two.
              points: { show: aligned.xs.length < 3, size: 8, width: 2, stroke: colors.surface, fill: stroke },
              ...(lines.length === 1 ? { fill: withAlpha(stroke, WASH) } : {}),
            };
          }),
        ],
        hooks: {
          setCursor: [
            (self) => {
              const index = self.cursor.idx;
              const left = self.cursor.left;
              if (index === null || index === undefined || left === undefined || left < 0) setHover((before) => (before?.fromKeyboard === true ? before : null));
              else setHover({ index, left: left + self.over.offsetLeft, fromKeyboard: false });
            },
          ],
        },
      };
      chart.current?.destroy();
      element.replaceChildren();
      chart.current = new UPlot(options, [aligned.xs.map((ms) => ms / 1000), ...aligned.ys], element);
    });
    return () => {
      cancelled = true;
      chart.current?.destroy();
      chart.current = null;
      element.replaceChildren();
    };
    // The chart is made again when its data, its span or the look of the page changes, and not when it is resized.
  }, [ready, aligned, lines, format, theme, locale, timeZone, from, to, height]);

  useEffect(() => {
    if (width > 0) chart.current?.setSize({ width, height });
  }, [width, height]);

  const move = (event: KeyboardEvent<HTMLDivElement>) => {
    const instance = chart.current;
    if (instance === null || aligned.xs.length === 0) return;
    const last = aligned.xs.length - 1;
    const current = hover?.index ?? nearestIndex(aligned.xs, to) ?? last;
    const next =
      event.key === 'ArrowLeft' ? Math.max(0, current - 1) : event.key === 'ArrowRight' ? Math.min(last, current + 1) : event.key === 'Home' ? 0 : event.key === 'End' ? last : null;
    if (next === null) return;
    event.preventDefault();
    const left = instance.valToPos((aligned.xs[next] ?? 0) / 1000, 'x');
    setHover({ index: next, left: left + instance.over.offsetLeft, fromKeyboard: true });
    instance.setCursor({ left, top: 0 }, false);
  };

  const leave = () => {
    chart.current?.setCursor({ left: -10, top: -10 }, false);
    setHover(null);
  };

  const when = useMemo(() => readoutTime(locale, timeZone), [locale, timeZone]);
  const rows = useMemo(() => sampleRows(aligned.xs.length), [aligned.xs.length]);
  const only = lines.length === 1 ? lines[0] : undefined;
  const largest = largestOf(aligned);

  // The readout leans to the side that has room: left of the crosshair in the right half of the plot.
  const readoutSide = hover !== null && width > 0 && hover.left > width / 2 ? 'right' : 'left';

  return (
    <figure className={cx('flex min-w-0 flex-col gap-2', className)} aria-busy={stale}>
      <div className={cx('relative min-w-0 transition-opacity', stale && 'opacity-60')}>
        <div
          ref={host}
          role="group"
          tabIndex={hasPoints ? 0 : -1}
          aria-label={`${label}. ${words.keys}`}
          onKeyDown={move}
          onBlur={leave}
          className="rounded-sm"
          style={{ height }}
        />
        {!hasPoints && (
          <div className="absolute inset-0 flex items-center justify-center px-4 text-center text-sm text-text-muted" role="status">
            {empty}
          </div>
        )}
        {hover !== null && hasPoints && (
          <div
            role="status"
            aria-live={hover.fromKeyboard ? 'polite' : 'off'}
            className="pointer-events-none absolute top-1 z-(--layer-sticky) flex min-w-32 flex-col gap-1 rounded-md border border-border bg-surface-raised px-2 py-2 text-sm shadow-md"
            style={readoutSide === 'left' ? { left: hover.left + READOUT_GAP } : { right: width - hover.left + READOUT_GAP }}
          >
            <span className="text-xs text-text-muted">{when(aligned.xs[hover.index] ?? 0)}</span>
            {lines.map((line, at) => {
              const value = aligned.ys[at]?.[hover.index] ?? null;
              return (
                <span key={line.id} className="flex items-center gap-2">
                  <LineKey slot={line.slot} />
                  <span className="font-semibold text-text tabular-nums">{value === null ? '–' : format(value)}</span>
                  <span className="min-w-0 truncate text-text-muted">{line.label}</span>
                </span>
              );
            })}
          </div>
        )}
      </div>

      {hasPoints && (
        <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
          {lines.map((line, at) => {
            const { now, peak } = figuresOf(aligned.ys[at] ?? []);
            return (
              <li key={line.id} className="flex min-w-0 items-center gap-2">
                {only === undefined && (
                  <>
                    <LineKey slot={line.slot} />
                    <span className="min-w-0 truncate text-text">{line.label}</span>
                  </>
                )}
                <span className="text-text-muted tabular-nums">
                  {words.now} {now === null ? '–' : format(now)}
                  {' · '}
                  {words.peak} {peak === null ? '–' : format(peak)}
                </span>
              </li>
            );
          })}
        </ul>
      )}

      {hasPoints && largest !== null && (
        <details className="text-sm text-text-muted">
          <summary className="w-fit cursor-default rounded-sm select-none hover:text-text">{words.table}</summary>
          <Preview more={rows.length > PREVIEW_LINES}>{(expanded) => <div className="mt-2 rounded-md border border-border">
            <table aria-label={`${label}: ${words.table}`} className="w-full table-fixed border-collapse text-left text-sm tabular-nums">
              <thead>
                <tr className="border-b border-border bg-bg-subtle text-text-muted">
                  <th scope="col" className="wrap-anywhere px-2 py-1 font-medium">
                    {words.time}
                  </th>
                  {lines.map((line) => (
                    <th key={line.id} scope="col" className="wrap-anywhere px-2 py-1 font-medium">
                      {line.label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {(expanded ? rows : rows.slice(0, PREVIEW_LINES)).map((index) => (
                  <tr key={aligned.xs[index]} className="border-b border-border last:border-b-0">
                    <th scope="row" className="wrap-anywhere px-2 py-1 font-normal text-text">
                      {when(aligned.xs[index] ?? 0)}
                    </th>
                    {lines.map((line, at) => {
                      const value = aligned.ys[at]?.[index] ?? null;
                      return (
                        <td key={line.id} className="wrap-anywhere px-2 py-1 text-text">
                          {value === null ? '–' : format(value)}
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>}</Preview>
        </details>
      )}
    </figure>
  );
}

/** The short stroke of a line's colour that keys it in the legend and the readout: a mark and never a box. */
export function LineKey({ slot }: { slot: number }) {
  return <span aria-hidden className={cx('inline-block h-0.5 w-4 shrink-0 rounded-full', KEY_CLASSES[(slot - 1) % SLOTS])} />;
}
