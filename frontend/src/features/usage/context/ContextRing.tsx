/** How full a thread's context is, at a glance: a ring beside the message controls, coloured by how near the conversation is to being shortened, with a tick where it is; one click opens what fills it.
 *
 * The ring is the share of the model's window that the last prompt filled, as the harness reported it:
 * green below 60 %, amber up to 85 %, red above. Its words, for the pointer and for a screen reader, read
 * "71 % of context · 142k of 200k · shortened at 90 %". Where the harness reports no size, there is no ring.
 */
import type { TFunction } from 'i18next';
import type { ComponentPropsWithRef } from 'react';
import { useTranslation } from 'react-i18next';

import { ContextPanel } from '@/features/usage/context/ContextPanel';
import { type ContextTone, type Ring, ringOf } from '@/features/usage/context/fill';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatPercent, formatTokens } from '@/lib/format';
import { cx } from '@/ui/cx';
import { Popover } from '@/ui/Popover';
import { Tooltip } from '@/ui/Tooltip';

const TONES: Record<ContextTone, string> = { green: 'text-success', amber: 'text-warning', red: 'text-danger' };

const RADIUS = 7;
const CIRCLE = 2 * Math.PI * RADIUS;
/** Where the tick begins and ends, from the centre: across the ring's stroke. */
const TICK_FROM = 4.5;
const TICK_TO = 9;
/** The most a ring's figure says: a harness past its window is past it, by however much. */
const FIGURE_MOST_PERCENT = 999;

/** The ring itself, with the tick where the conversation is shortened. */
export function RingMark({ ring, className }: { ring: Ring; className?: string }) {
  const held = Math.min(100, Math.max(0, ring.percent));
  const tick = ring.compactPercent === null ? null : ((ring.compactPercent / 100) * 360 - 90) * (Math.PI / 180);
  return (
    <svg aria-hidden viewBox="0 0 20 20" className={cx('size-4 shrink-0', TONES[ring.tone], className)} data-tone={ring.tone}>
      <circle cx="10" cy="10" r={RADIUS} fill="none" stroke="currentColor" strokeOpacity="0.25" strokeWidth="2.5" />
      <circle
        cx="10"
        cy="10"
        r={RADIUS}
        fill="none"
        stroke="currentColor"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeDasharray={CIRCLE}
        strokeDashoffset={CIRCLE * (1 - held / 100)}
        transform="rotate(-90 10 10)"
      />
      {tick !== null && (
        <line
          data-ring-tick
          x1={10 + TICK_FROM * Math.cos(tick)}
          y1={10 + TICK_FROM * Math.sin(tick)}
          x2={10 + TICK_TO * Math.cos(tick)}
          y2={10 + TICK_TO * Math.sin(tick)}
          stroke="var(--text)"
          strokeWidth="1.5"
        />
      )}
    </svg>
  );
}

/** The words of a ring: its share, its tokens of the window and where the conversation is shortened. */
export function ringWords(ring: Ring, t: TFunction, locale: string): string {
  return t(ring.compactPercent === null ? 'contextUsage.ring.labelBare' : 'contextUsage.ring.label', {
    percent: formatPercent(ring.percent / 100, locale),
    used: formatTokens(ring.used, locale),
    window: formatTokens(ring.window, locale),
    compact: formatPercent((ring.compactPercent ?? 0) / 100, locale),
  });
}

/** The button the ring is: what a popover and a tooltip hand down reaches the element itself. */
function RingButton({ ring, words, ...handedDown }: { ring: Ring; words: string } & ComponentPropsWithRef<'button'>) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return (
    <button
      type="button"
      {...handedDown}
      aria-label={t('contextUsage.ring.open', { label: words })}
      className="inline-flex items-center gap-1 rounded-md px-1 py-1 text-xs text-text-muted tabular-nums hover:bg-bg-muted"
      data-context-ring={ring.tone}
    >
      <RingMark ring={ring} />
      <span aria-hidden>{formatPercent(Math.min(ring.percent, FIGURE_MOST_PERCENT) / 100, locale)}</span>
    </button>
  );
}

export function ContextRing({ thread, side = 'bottom' }: { thread: Thread; side?: 'top' | 'bottom' }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const ring = ringOf(thread.usage);
  if (ring === null) return null;
  const words = ringWords(ring, t, locale);
  return (
    <Popover
      label={t('contextUsage.panel.title')}
      side={side}
      align="start"
      className="max-h-(--radix-popover-content-available-height) overflow-y-auto"
      trigger={
        <Tooltip content={words}>
          <RingButton ring={ring} words={words} />
        </Tooltip>
      }
    >
      <ContextPanel thread={thread} />
    </Popover>
  );
}
