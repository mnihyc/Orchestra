/** Shared Sparkline component (measures); primitives are wrapped here.
 *
 * A small chart of how something moved over a stretch of time: one line or two, filled under the
 * first, with a dashed line where a limit stands. It is drawn as a picture that is named for a screen
 * reader by what it shows (`label`), and the owner puts the figures it stands for in words beside it,
 * since a line alone says nothing of its values. The vertical scale begins at zero and reaches the
 * highest value or the limit, whichever is higher, so that two charts of the same thing can be compared.
 */
import { useId } from 'react';

import { cx } from '@/ui/cx';

export type SparkTone = 'accent' | 'success' | 'warning' | 'danger' | 'info';

export interface SparkSeries {
  /** Null keeps an unmeasured interval as a gap, at its original time position. */
  values: readonly (number | null)[];
  tone?: SparkTone;
}

export interface SparklineProps {
  /** One or two series of equal length, oldest value first. */
  series: readonly SparkSeries[];
  /** Where a limit stands, in the same unit as the values. */
  limit?: number | null;
  /** The highest value the vertical scale reaches; the highest value of the series and the limit when absent. */
  max?: number;
  /** What the picture shows, for a screen reader. */
  label: string;
  className?: string;
}

const STROKE: Record<SparkTone, string> = {
  accent: 'stroke-accent',
  success: 'stroke-success',
  warning: 'stroke-warning',
  danger: 'stroke-danger',
  info: 'stroke-info',
};
const FILL: Record<SparkTone, string> = {
  accent: 'fill-accent/15',
  success: 'fill-success/15',
  warning: 'fill-warning/15',
  danger: 'fill-danger/15',
  info: 'fill-info/15',
};

/** The picture is drawn in a box this wide and tall and scaled to what it lies in. */
const WIDTH = 100;
const HEIGHT = 24;

export function Sparkline({ series, limit = null, max, label, className }: SparklineProps) {
  const id = useId();
  const longest = Math.max(1, ...series.map((one) => one.values.length));
  const peak = Math.max(1e-9, max ?? 0, limit ?? 0, ...series.flatMap((one) => one.values.filter((value): value is number => value !== null)));
  const x = (index: number) => (longest === 1 ? WIDTH : (index / (longest - 1)) * WIDTH);
  const y = (value: number) => HEIGHT - (Math.min(value, peak) / peak) * HEIGHT;
  return (
    <svg
      role="img"
      aria-label={label}
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      preserveAspectRatio="none"
      className={cx('block h-(--chart-height) w-full overflow-visible rounded-sm bg-bg-subtle', className)}
    >
      {series.map((one, at) => {
        if (one.values.length === 0) return null;
        const tone = one.tone ?? 'accent';
        const runs: { first: number; last: number; points: string[] }[] = [];
        one.values.forEach((value, index) => {
          if (value === null) return;
          let run = runs.at(-1);
          if (run?.last !== index - 1) { run = { first: index, last: index, points: [] }; runs.push(run); }
          run.last = index;
          run.points.push(`${x(index).toFixed(2)},${y(value).toFixed(2)}`);
        });
        return (
          <g key={`${id}.${at}`}>
            {runs.map((run) => <g key={run.first}>{at === 0 && <polygon className={FILL[tone]} points={`${x(run.first)},${HEIGHT} ${run.points.join(' ')} ${x(run.last).toFixed(2)},${HEIGHT}`} />}<polyline className={cx('fill-none', STROKE[tone])} strokeWidth={1.5} vectorEffect="non-scaling-stroke" strokeLinejoin="round" points={run.points.join(' ')} /></g>)}
          </g>
        );
      })}
      {limit !== null && limit <= peak && (
        <line x1={0} x2={WIDTH} y1={y(limit)} y2={y(limit)} className="stroke-danger" strokeWidth={1} strokeDasharray="3 2" vectorEffect="non-scaling-stroke" />
      )}
    </svg>
  );
}
