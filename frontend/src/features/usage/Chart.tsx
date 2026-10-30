/** The series over the span, by the hour, day, week or month: the measure shown and the dimension it is split by; dragging across it sets the span.
 *
 * Bars drawn by uPlot on a canvas that is loaded when the first chart is shown and by nothing else, one
 * stack for each member of the dimension the series is split by, each keeping its colour for what it is
 * of. Hovering a bar reads its parts; the arrow keys move the same readout, and Enter on a bar sets the
 * span to it, as dragging across bars does. The values are also in a table under the chart, so that
 * nothing is told by colour or by the pointer alone, and a span with no use draws no chart: it says so.
 * Everything is drawn in UTC, which is the clock the ledger is kept by.
 */
import type uPlot from 'uplot';
import { type KeyboardEvent, type ReactNode, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatMeasure, formatScale } from '@/features/usage/figures';
import { assignSlots, type Bars, REST_SLOT, SLOTS } from '@/features/usage/series';
import type { Measure } from '@/features/usage/search';
import { useAppearance } from '@/lib/appearance';
import { cx } from '@/ui/cx';
import { nearestIndex, sampleRows, scaleTop } from '@/ui/timeSeries';
import { Table, type Column } from '@/ui/Table';
import { Button } from '@/ui/Button';
import { type Look } from './context/fill';
import { Swatch } from './context/PartsBar';

export interface UsageChartProps {
  /** Shares the actual stable series colours with the adjacent breakdown tables. */
  onColors?: (colors:ReadonlyMap<string,number>)=>void;
  /** What is plotted: it names the chart for a screen reader and heads the table. */
  label: string;
  bars: Bars;
  measure: Measure;
  /** Sets the span to the days between two moments: what dragging across the bars, or Enter on one, does. */
  onSelect?: ((fromMs: number, toMs: number) => void) | undefined;
  /** The answer for another span is on its way: the render that is there is held, dimmed. */
  stale?: boolean;
  height?: number;
  className?: string;
  /** What stands in the chart's place while the span has no use. */
  empty: ReactNode;
}

const DEFAULT_HEIGHT = 200;

/** The plotting code once it has been fetched: a chart that is made after the first stands in the commit that asks for it, without waiting for a promise. */
let plotting: typeof uPlot | null = null;

/** The height of the band of the time scale, and the fewest pixels between two of its labels. */
const TIME_AXIS_SIZE = 26;
const TIME_TICK_SPACE = 90;

/** Room round the widest figure of the value axis, in pixels. */
const AXIS_PADDING = 16;

/** Where the readout stands relative to the crosshair, in pixels. */
const READOUT_GAP = 12;

/** How wide a bar is of the room its bucket has, and the widest it gets, in pixels: a bar leaves a little air on either side. */
const BAR_SHARE = 0.78;
const BAR_WIDEST_PX = 56;

/** Token hatching repeats at the same four-pixel spacing as the shared part bar. */
const TOKEN_PATTERN_TILE = 4;
/** A larger tile carries both diagonal strokes without a visible seam. */
const TOKEN_DIAGONAL_TILE = 10;

function tokenPattern(element:HTMLElement,look:Look,ink:string):string | CanvasPattern {
  if(look.pattern==='solid') return ink;
  const tile=look.pattern==='diagonal' || look.pattern==='cross' ? TOKEN_DIAGONAL_TILE : TOKEN_PATTERN_TILE;
  const ratio=window.devicePixelRatio || 1;
  const canvas=element.ownerDocument.createElement('canvas');
  canvas.width=tile*ratio; canvas.height=tile*ratio;
  const context=canvas.getContext('2d');
  if(context===null) return ink;
  context.scale(ratio,ratio);
  context.fillStyle=ink; context.fillRect(0,0,tile,tile);
  context.fillStyle='rgb(255 255 255 / 0.45)';
  context.strokeStyle=context.fillStyle;
  if(look.pattern==='stripes' || look.pattern==='grid') context.fillRect(0,0,1,tile);
  if(look.pattern==='grid') context.fillRect(0,0,tile,1);
  if(look.pattern==='dots') { context.beginPath(); context.arc(tile/2,tile/2,1,0,Math.PI*2); context.fill(); }
  if(look.pattern==='diagonal' || look.pattern==='cross') {
    context.lineWidth=1.5;
    for(const offset of [-tile,0,tile]) {
      context.beginPath(); context.moveTo(offset,0); context.lineTo(offset+tile,tile); context.stroke();
      if(look.pattern==='cross') { context.beginPath();context.moveTo(offset,tile);context.lineTo(offset+tile,0);context.stroke(); }
    }
  }
  return context.createPattern(canvas,'repeat') ?? ink;
}

/** A drag shorter than this many pixels is a click, which sets nothing. */
const DRAG_LEAST_PX = 4;

/** Most rows the table of a chart opens with; a longer one scrolls. */
const TABLE_HEIGHT = 'max-h-[18rem]';

const KEY_CLASSES = [
  'bg-text-subtle',
  'bg-chart-1',
  'bg-chart-2',
  'bg-chart-3',
  'bg-chart-4',
  'bg-chart-5',
  'bg-chart-6',
  'bg-chart-7',
  'bg-chart-8',
] as const;

/** The small square that keys a stack in the legend and the readout: a mark, never the only thing that says which. */
export function StackKey({ slot, look }: { slot: number; look?:Look | undefined }) {
  return look===undefined ? <span aria-hidden className={cx('inline-block size-3 shrink-0 rounded-sm',KEY_CLASSES[slot % (SLOTS + 1)])} /> : <Swatch segment={{look}} />;
}

/** A value of a stack as it is read: use that had no rate is "–" in credits, never 0. */
function stackValue(value: number, unrated: boolean, measure: Measure, locale: string): string {
  return measure === 'credits' && unrated && value === 0 ? '–' : formatMeasure(value, measure, locale);
}

interface Colors {
  slots: string[];
  errors: string[];
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
    // The quiet colour first, then the eight the design gives.
    slots: [read('--text-subtle'), ...Array.from({ length: SLOTS }, (_, slot) => read(`--chart-${slot + 1}`))],
    errors: [read('--warning'), read('--danger')],
    grid: read('--border'),
    axis: read('--border-strong'),
    ink: read('--text-muted'),
    surface: read('--surface'),
    font: `12px ${css.fontFamily}`,
  };
}

interface Hover {
  index: number;
  left: number;
  fromKeyboard: boolean;
}

function formatters(locale: string, bucket: Bars['bucket'], spanMs: number) {
  const make = (options: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat(locale, { ...options, timeZone: 'UTC' });
  const tick =
    bucket === 'hour' ? make({ hour: 'numeric', minute: '2-digit' }) : bucket === 'month' ? make({ month: 'short', year: '2-digit' }) : spanMs > 400 * 86_400_000 ? make({ month: 'short', year: 'numeric' }) : make({ month: 'short', day: 'numeric' });
  const reading =
    bucket === 'hour' ? make({ dateStyle: 'medium', timeStyle: 'short' }) : bucket === 'month' ? make({ month: 'long', year: 'numeric' }) : make({ weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' });
  return { tick: (stamp: number) => tick.format(new Date(stamp * 1000)), reading: (ms: number) => reading.format(new Date(ms)) };
}

export function UsageChart({ label, bars, measure, onSelect, stale = false, height = DEFAULT_HEIGHT, className, empty, onColors }: UsageChartProps) {
  const { t } = useTranslation();
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);
  const [kept, setKept] = useState<ReadonlyMap<string, number>>(new Map());
  const theme = useAppearance((state) => state.resolvedTheme);
  const locale = useAppearance((state) => state.locale);
  const [hover, setHover] = useState<Hover | null>(null);
  const [width, setWidth] = useState(0);
  const [tableOpen, setTableOpen] = useState(false);
  const tableId = useId();
  const selectable = onSelect !== undefined;
  const latestSelect = useRef(onSelect);
  useEffect(() => {
    latestSelect.current = onSelect;
  }, [onSelect]);

  const printed = (value: number, unrated: boolean, count?: number) => measure === 'errors' && Number.isFinite(value) && count !== undefined ? t('usagePerf.rateCount', { rate: formatMeasure(value, measure, locale), errors: new Intl.NumberFormat(locale).format(count) }) : stackValue(value, unrated, measure, locale);
  const hasUse = bars.performance ? bars.heights.some(Number.isFinite) || bars.stacks.some((stack) => stack.values.some(Number.isFinite)) : bars.heights.some((height) => height > 0);
  const showParts = bars.stacks.length > 1 || (bars.stacks.length===1 && bars.stacks[0]?.id!=='all');
  const legend=bars.stacks.filter((stack)=>stack.values.some((value)=>Number.isFinite(value) && (bars.performance===true || value>0)));
  // The colours the stacks keep, given in the order they are drawn and kept while the chart is split the same way.
  const colorOf = useMemo(() => assignSlots(kept, bars.stacks), [kept, bars.stacks]);
  // What was given is kept for the next answer: set while drawing, and only when it differs, which is how a value from before is carried forward.
  if (colorOf.size !== kept.size || [...colorOf].some(([id, slot]) => kept.get(id) !== slot)) setKept(colorOf);
  useEffect(()=>{onColors?.(colorOf);},[onColors,colorOf]);

  // How wide the room is; the chart is made once there is some, and resized after that.
  useEffect(() => {
    const element = host.current;
    if (element === null) return;
    setWidth(element.clientWidth);
    const watch = new ResizeObserver(() => setWidth(element.clientWidth));
    watch.observe(element);
    return () => watch.disconnect();
  }, []);

  const first = bars.xs[0];
  const lastIndex = bars.xs.length - 1;
  const last = bars.xs[lastIndex];
  const lastWidth = bars.widths[lastIndex] ?? 0;
  const spanMs = first === undefined || last === undefined ? 0 : last + lastWidth - first;
  // A bar stands in the middle of its bucket.
  const centers = useMemo(() => bars.xs.map((start, at) => start + (bars.widths[at] ?? 0) / 2), [bars.xs, bars.widths]);
  const ready = width > 0 && hasUse;

  // Made before the browser paints, in the commit that has its data: what is read is then drawn in the frame it arrives in.
  useLayoutEffect(() => {
    const element = host.current;
    if (element === null || !ready || first === undefined || last === undefined) return;
    let cancelled = false;
    const build = (UPlot: typeof uPlot) => {
      const colors = readColors(element);
      const words = formatters(locale, bars.bucket, spanMs);
      const endLabel = words.tick((last + lastWidth) / 1000);
      const barsPath = UPlot.paths.bars;
      let widestSeen = AXIS_PADDING + 24;
      let endRoom = 0;
      let marked = false;

      // uPlot draws in backing pixels with a rounded, DPR-scaled font. Axis sizes and
      // padding are CSS pixels. Measure that same font and restore its drawing state.
      const labelRoom = (self: uPlot, values: readonly string[] | null, align: 'center' | 'right') => {
        const ratio = UPlot.pxRatio;
        self.ctx.save();
        self.ctx.font = colors.font.replace(/^\d+px/, (size) => `${Math.round(Number.parseFloat(size) * ratio)}px`);
        self.ctx.textAlign = align;
        let room = 0;
        for (const value of values ?? []) {
          const metrics = self.ctx.measureText(value);
          room = Math.max(room, align === 'center' ? metrics.width / 2 : metrics.width,
            metrics.actualBoundingBoxLeft, metrics.actualBoundingBoxRight);
        }
        self.ctx.restore();
        return Math.ceil(room / ratio);
      };

      // Stacks are drawn from the tallest sum to the lowest, each over the one before it.
      const stacks = bars.stacks;
      const sums: number[][] = [];
      let running = Array.from({ length: bars.xs.length }, () => 0);
      for (const stack of stacks) {
        running = running.map((sum, at) => sum + (stack.values[at] ?? 0));
        sums.push(running);
      }
      const line = bars.lines === true;
      const drawn = line ? stacks.map((stack) => stack.values) : sums.map((_, at) => sums[stacks.length - 1 - at] ?? []);
      const drawnStacks = line ? stacks : stacks.map((_, at) => stacks[stacks.length - 1 - at]);
      const color = (stack: typeof stacks[number] | undefined) => stack?.look===undefined ? colors.slots[colorOf.get(stack?.id ?? '') ?? REST_SLOT] ?? colors.ink : getComputedStyle(element).getPropertyValue(stack.look.colour.slice(4,-1)).trim();

      const options: uPlot.Options = {
        width: element.clientWidth,
        height,
        legend: { show: false },
        padding: [8, () => endRoom, 0, 0],
        tzDate: (stamp) => UPlot.tzDate(new Date(stamp * 1000), 'UTC'),
        cursor: {
          x: true,
          y: false,
          points: { show: false },
          drag: { x: selectable, y: false, setScale: false },
        },
        select: { show: selectable, left: 0, top: 0, width: 0, height: 0 },
        scales: {
          x: { time: true, range: [first / 1000, (last + lastWidth) / 1000] },
          y: { range: (_chart, _min, max) => [0, scaleTop(max)] },
        },
        axes: [
          {
            stroke: colors.ink,
            font: colors.font,
            size: (self, values: readonly string[] | null) => {
              // Seed the endpoint before uPlot's first layout, then retain the widest
              // actual split label as it converges or resizes, including glyph overhang.
              // uPlot's initialization passes null despite its narrower declaration.
              endRoom = Math.max(endRoom, labelRoom(self, [...(values ?? []), endLabel], 'center'));
              return TIME_AXIS_SIZE;
            },
            gap: 4,
            space: TIME_TICK_SPACE,
            grid: { stroke: colors.grid, width: 1 },
            ticks: { stroke: colors.grid, width: 1, size: 4 },
            border: { stroke: colors.axis, width: 1 },
            values: (_chart, splits) => splits.map((stamp) => words.tick(stamp)),
          },
          {
            stroke: colors.ink,
            font: colors.font,
            grid: { stroke: colors.grid, width: 1 },
            ticks: { show: false },
            border: { show: false },
            values: (_chart, splits) => splits.map((value) => formatScale(value, measure, locale)),
            size: (self, values, _axisIndex, cycle) => {
              if (cycle > 1) return widestSeen;
              widestSeen = labelRoom(self, values, 'right') + AXIS_PADDING;
              return widestSeen;
            },
          },
        ],
        bands: [],
        series: [
          {},
          ...drawnStacks.map((stack): uPlot.Series => {
            const ink = color(stack);
            if (line) return { label: stack?.label ?? '', stroke: ink, width: 2, points: { show: true,size:4,width:0,fill:ink }, spanGaps: false };
            return {
              label: stack?.label ?? '',
              fill: stack?.look===undefined ? ink : tokenPattern(element,stack.look,ink),
              // A line of the surface between two stacks, so that neighbours are told apart by more than their colour.
              stroke: colors.surface,
              width: 1,
              paths: barsPath === undefined ? undefined : barsPath({ size: [BAR_SHARE, BAR_WIDEST_PX], radius: 0 }),
              points: { show: false },
            };
          }),
        ],
        hooks: {
          // The moment the bars are on the canvas, once for each chart made: the performance panel shows it, and the test of a year of days reads it.
          draw: [
            () => {
              if (marked) return;
              marked = true;
              performance.mark('usage-chart-drawn', { detail: { bars: centers.length, stacks: stacks.length } });
            },
          ],
          setCursor: [
            (self) => {
              const index = self.cursor.idx;
              const left = self.cursor.left;
              if (index === null || index === undefined || left === undefined || left < 0) setHover((before) => (before?.fromKeyboard === true ? before : null));
              else setHover({ index, left: left + self.over.offsetLeft, fromKeyboard: false });
            },
          ],
          setSelect: [
            (self) => {
              if (self.select.width < DRAG_LEAST_PX) return;
              const lo = self.posToVal(self.select.left, 'x') * 1000;
              const hi = self.posToVal(self.select.left + self.select.width, 'x') * 1000;
              const a = nearestIndex(centers, lo);
              const b = nearestIndex(centers, hi);
              self.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
              if (a === null || b === null) return;
              const [from, to] = a <= b ? [a, b] : [b, a];
              const start = bars.xs[from];
              const end = bars.xs[to];
              if (start === undefined || end === undefined) return;
              latestSelect.current?.(start, end + (bars.widths[to] ?? 0) - 1);
            },
          ],
        },
      };
      chart.current?.destroy();
      element.replaceChildren();
      const instance = new UPlot(options, [centers.map((ms) => ms / 1000), ...drawn.map((values) => values.map((n) => Number.isFinite(n) ? n : null))], element);
      chart.current = instance;
      // The shipped font uses font-display: swap. A chart drawn before it arrives
      // must remeasure after it settles, without reviving a replaced chart.
      void document.fonts.ready.then(() => {
        if (!cancelled && chart.current === instance) instance.redraw(false, true);
      });
    };
    if (plotting !== null) build(plotting);
    else {
      void Promise.all([import('uplot'), import('uplot/dist/uPlot.min.css')]).then(([module]) => {
        plotting = module.default;
        if (!cancelled) build(module.default);
      });
    }
    return () => {
      cancelled = true;
      chart.current?.destroy();
      chart.current = null;
      element.replaceChildren();
    };
    // Made again when its data or the look of the page changes, and not when it is resized.
  }, [ready, bars, centers, measure, theme, locale, height, selectable, colorOf, first, last, lastWidth, spanMs]);

  useEffect(() => {
    if (width > 0) chart.current?.setSize({ width, height });
  }, [width, height]);

  const move = (event: KeyboardEvent<HTMLDivElement>) => {
    const instance = chart.current;
    if (instance === null || bars.xs.length === 0) return;
    const lastBar = bars.xs.length - 1;
    const current = hover?.index ?? lastBar;
    if (event.key === 'Enter' && hover?.fromKeyboard === true && latestSelect.current !== undefined) {
      const start = bars.xs[hover.index];
      if (start === undefined) return;
      event.preventDefault();
      latestSelect.current(start, start + (bars.widths[hover.index] ?? 0) - 1);
      return;
    }
    const next =
      event.key === 'ArrowLeft' ? Math.max(0, current - 1) : event.key === 'ArrowRight' ? Math.min(lastBar, current + 1) : event.key === 'Home' ? 0 : event.key === 'End' ? lastBar : null;
    if (next === null) return;
    event.preventDefault();
    const left = instance.valToPos((centers[next] ?? 0) / 1000, 'x');
    setHover({ index: next, left: left + instance.over.offsetLeft, fromKeyboard: true });
    instance.setCursor({ left, top: 0 }, false);
  };

  const leave = () => {
    chart.current?.setCursor({ left: -10, top: -10 }, false);
    setHover(null);
  };

  const words = useMemo(() => formatters(locale, bars.bucket, spanMs), [locale, bars.bucket, spanMs]);
  const readoutSide = hover !== null && width > 0 && hover.left > width / 2 ? 'right' : 'left';
  const rows = useMemo(() => sampleRows(bars.xs.length, bars.xs.length), [bars.xs.length]);
  const overallHighs=!showParts ? bars.stacks[0]?.highs : undefined;
  const columns: Column<number>[] = [
    { id: 'time', header: t('usage.chart.time'), cell: (index) => words.reading(bars.xs[index] ?? 0) },
    ...(showParts ? bars.stacks.map<Column<number>>((stack) => ({ id: `part:${stack.id}`, header: stack.label, align: 'end', cell: (index) => <span className="tabular-nums">{printed(stack.values[index] ?? Number.NaN, stack.unrated[index] ?? false, stack.counts?.[index])}{stack.highs!==undefined && <span className="block text-xs text-text-muted">{t('usagePerf.percentile', { value: formatMeasure(stack.highs[index] ?? Number.NaN, measure, locale) })}</span>}</span> })) : []),
    { id: 'total', header: t(bars.lines ? 'usagePerf.overall' : 'usage.chart.total'), align: 'end', cell: (index) => <span className="tabular-nums">{printed(bars.heights[index] ?? Number.NaN, false, bars.counts?.[index])}{overallHighs!==undefined && <span className="block text-xs text-text-muted">{t('usagePerf.percentile',{value:formatMeasure(overallHighs[index] ?? Number.NaN,measure,locale)})}</span>}</span> },
  ];

  return (
    <figure className={cx('flex min-w-0 flex-col gap-2', className)} aria-busy={stale}>
      <div className={cx('relative min-w-0 transition-opacity', stale && 'opacity-60')}>
        <div
          ref={host}
          role="group"
          aria-hidden={!hasUse}
          tabIndex={hasUse ? 0 : -1}
          aria-label={`${label}. ${t('usage.chart.keys')}`}
          onKeyDown={move}
          onBlur={leave}
          className="rounded-sm [&_.u-cursor-x]:border-text-subtle! [&_.u-select]:bg-accent-subtle [&_.u-select]:opacity-70"
          style={{ height: hasUse ? height : 0 }}
        />
        {!hasUse && (
          <p role="status" className="text-sm text-text-muted">{empty}</p>
        )}
        {hover !== null && hasUse && (
          <div
            role="status"
            aria-live={hover.fromKeyboard ? 'polite' : 'off'}
            className="pointer-events-none absolute top-1 z-(--layer-sticky) flex min-w-[10rem] flex-col gap-1 rounded-md border border-border bg-surface-raised px-2 py-2 text-sm shadow-md"
            style={readoutSide === 'left' ? { left: hover.left + READOUT_GAP } : { right: width - hover.left + READOUT_GAP }}
          >
            <span className="text-xs text-text-muted">{words.reading(bars.xs[hover.index] ?? 0)}</span>
            <span className="flex items-baseline justify-between gap-3">
              <span className="text-text-muted">{t(bars.lines ? 'usagePerf.overall' : 'usage.chart.total')}</span>
              <span className="font-semibold text-text tabular-nums">{printed(bars.heights[hover.index] ?? Number.NaN, false, bars.counts?.[hover.index])}</span>
            </span>
            {showParts &&
              bars.stacks
                .map((stack) => ({ stack, value: stack.values[hover.index] ?? 0, unrated: stack.unrated[hover.index] ?? false }))
                .filter(({ value, unrated }) => bars.performance ? Number.isFinite(value) : value > 0 || (unrated && measure === 'credits'))
                .map(({ stack, value, unrated }) => (
                  <span key={stack.id} className="flex items-center gap-2">
                    <StackKey slot={colorOf.get(stack.id) ?? REST_SLOT} look={stack.look} />
                    <span className="min-w-0 flex-1 truncate text-text-muted">{stack.label}</span>
                    <span className="text-text tabular-nums">{printed(value, unrated, stack.counts?.[hover.index])}{stack.highs!==undefined && <span className="block text-xs text-text-muted">{t('usagePerf.percentile', { value: formatMeasure(stack.highs[hover.index] ?? Number.NaN, measure, locale) })}</span>}</span>
                  </span>
                ))}
          </div>
        )}
      </div>

      {hasUse && showParts && legend.length>0 && (
        <ul aria-label={t('usage.chart.legend')} className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
          {legend.map((stack) => (
            <li key={stack.id} className="flex min-w-0 items-center gap-2">
              <StackKey slot={colorOf.get(stack.id) ?? REST_SLOT} look={stack.look} />
              <span className="min-w-0 truncate text-text" title={stack.detail ?? undefined}>
                {stack.label}
              </span>
              <span className="text-text-muted tabular-nums">{printed(stack.total, stack.allUnrated, stack.count)}</span>
            </li>
          ))}
        </ul>
      )}

      {hasUse && (
        <div className="flex min-w-0 flex-col gap-2">
          <Button size="sm" variant="ghost" className="self-start" aria-expanded={tableOpen} aria-controls={tableId} onClick={() => setTableOpen((open) => !open)}>{t('usage.chart.table')}</Button>
          {tableOpen && (
            <div id={tableId} role="region" aria-label={t('usage.chart.table')} tabIndex={0} data-scroll-allow="table" className={cx('overflow-auto', TABLE_HEIGHT)}>
              <Table className="overflow-x-auto!" label={`${label}: ${t('usage.chart.table')}`} columns={columns} rows={rows} rowKey={(index) => String(bars.xs[index])} />
            </div>
          )}
        </div>
      )}
    </figure>
  );
}
