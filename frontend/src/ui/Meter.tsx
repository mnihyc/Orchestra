/** Shared Meter component (measures); primitives are wrapped here.
 *
 * How full something is, in one control and four looks. `bar` (the default) is a bar with its figure
 * written beside its label: a window of a plan, a share of a host's memory; `compact` is the same
 * without the label, for a cell of a table. `ring` is a small mark for a header (how much of the
 * model's context is used), and `progress` a bare bar for work that is under way (how far a switch has
 * come), whose figure the page writes in its own words. The figure is in percent, held to 0 to 100.
 * What is full is drawn as a warning from 80 % and as danger from 95 %, except work under way, which is
 * not a warning for being half done; and the colour never stands alone: the figure is written, or
 * it is in the name a screen reader says.
 */
import type { ReactNode } from 'react';

import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { cx } from '@/ui/cx';

/** From this share what is used is drawn as a warning: what is left is getting short. */
export const METER_WARN_FROM = 80;
/** From this share it is drawn as danger: there is almost nothing left. */
export const METER_DANGER_FROM = 95;

/** How full a figure is, for whatever draws it: a bar, a ring, or the words of a chip. */
export type Fullness = 'low' | 'warning' | 'danger';

export function fullness(percent: number): Fullness {
  return percent >= METER_DANGER_FROM ? 'danger' : percent >= METER_WARN_FROM ? 'warning' : 'low';
}

export type MeterLook = 'bar' | 'compact' | 'ring' | 'progress';

export interface MeterProps {
  /** How much is used, in percent. A figure outside 0 to 100 is held to it. */
  value: number;
  /** What is measured: spoken, and, in the `bar` look, written above the bar. For a ring or a progress it also says the figure. */
  label: string;
  /** The look: a bar with its label and figure (default), a bar and its figure alone for a cell, a ring, or the bare bar of work under way. */
  look?: MeterLook;
  /** One quiet line under the bar: when the window ends. Only in the `bar` look. */
  detail?: ReactNode;
  className?: string;
  /** Native observations use the accent; an actual reached limit is a separate state. */
  neutral?: boolean;
  /** Preserve a vendor's overage in the figure while capping the drawn track. */
  showActual?: boolean;
}

const FILLS: Record<Fullness, string> = { low: 'bg-accent', warning: 'bg-warning', danger: 'bg-danger' };
/** A ring is quiet until it fills: it stands in a header, among words. */
const RING_TONES: Record<Fullness, string> = { low: 'text-text-subtle', warning: 'text-warning', danger: 'text-danger' };

const RING_RADIUS = 7;
const RING_LENGTH = 2 * Math.PI * RING_RADIUS;

export function Meter({ value, label, look = 'bar', detail, className, neutral = false, showActual = false }: MeterProps) {
  const locale = useAppearance((state) => state.locale);
  const held = Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : 0;
  const shown = formatNumber(showActual && Number.isFinite(value) ? value : Math.round(held), locale, { maximumFractionDigits: 2 });
  const state = fullness(held);

  if (look === 'ring') {
    return (
      <svg
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(held)}
        viewBox="0 0 18 18"
        className={cx('size-4 shrink-0', RING_TONES[state], className)}
      >
        <circle cx="9" cy="9" r={RING_RADIUS} fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2.5" />
        <circle
          cx="9"
          cy="9"
          r={RING_RADIUS}
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeDasharray={RING_LENGTH}
          strokeDashoffset={RING_LENGTH * (1 - held / 100)}
          transform="rotate(-90 9 9)"
        />
      </svg>
    );
  }

  const progress = look === 'progress';
  // In a row the bar takes what the figure leaves; in a column it is as wide as its label, and its height is its own.
  const bar = (
    <span
      role={progress ? 'progressbar' : 'meter'}
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(held)}
      aria-valuetext={progress ? undefined : `${shown}%`}
      className={cx('block h-(--meter-bar) overflow-hidden rounded-full bg-bg-muted', look === 'compact' ? 'min-w-10 flex-1' : 'w-full', progress && className)}
    >
      <span className={cx('block h-full rounded-full', progress ? 'bg-accent transition-[width]' : neutral ? 'bg-accent' : FILLS[state])} style={{ width: `${held}%` }} />
    </span>
  );
  if (progress) return bar;
  if (look === 'compact') {
    return (
      <span className={cx('inline-flex min-w-[5rem] items-center gap-2 text-sm text-text-muted tabular-nums', className)}>
        {bar}
        <span aria-hidden>{shown}%</span>
      </span>
    );
  }
  return (
    <span className={cx('flex flex-col gap-1', className)}>
      <span className="flex items-baseline justify-between gap-2 text-sm">
        <span className="text-text">{label}</span>
        <span className="text-text-muted tabular-nums">{shown}%</span>
      </span>
      {bar}
      {detail !== undefined && <span className="text-xs text-text-muted">{detail}</span>}
    </span>
  );
}
