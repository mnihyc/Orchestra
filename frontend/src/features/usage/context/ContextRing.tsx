/** How full a thread's context is, at a glance: a ring beside the message controls, coloured by how near the conversation is to being shortened, with a tick where it is; one click opens what fills it, and, where the model has more than one size of context, the choice of the size.
 *
 * The ring is the share of the model's window that the last prompt filled, as the harness reported it:
 * green below 60 %, amber up to 85 %, red above. Its words, for the pointer and for a screen reader, read
 * "71 % of context · 142k of 200k · shortened at 90 %". Where the harness reports no window, the tokens used stand
 * in its place; where it has reported nothing yet and the model has more than one size, the size in force does.
 */
import type { TFunction } from 'i18next';
import { Gauge } from 'lucide-react';
import type { ComponentPropsWithRef } from 'react';
import { useTranslation } from 'react-i18next';

import { ContextChoice } from '@/features/composer/ContextChoice';
import { PickList } from '@/features/composer/PickList';
import { sizeRows, WithheldNote, withheldOf } from '@/features/composer/Withheld';
import type { ContextState } from '@/features/composer/modelState';
import { useRunsWith } from '@/features/composer/runsWith';
import { contextChoices, formatContext } from '@/features/models/context';
import { modelDisplayName } from '@/features/models/names';
import { ContextPanel } from '@/features/usage/context/ContextPanel';
import { type ContextTone, type Ring, ringOf, contextModelName, usedAlone } from '@/features/usage/context/fill';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatPercent, formatTokens } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
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
function RingButton({ ring, used, words, ...handedDown }: { ring: Ring | null; used: number; words: string } & ComponentPropsWithRef<'button'>) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return (
    <button
      type="button"
      {...handedDown}
      aria-label={t('contextUsage.ring.open', { label: words })}
      className="inline-flex items-center gap-1 rounded-md px-1 py-1 text-xs text-text-muted tabular-nums hover:bg-bg-muted"
      data-context-ring={ring === null ? 'unmeasured' : ring.tone}
    >
      {ring === null ? <Gauge aria-hidden className="size-4 shrink-0" /> : <RingMark ring={ring} />}
      <span aria-hidden>{ring === null ? formatTokens(used, locale) : formatPercent(Math.min(ring.percent, FIGURE_MOST_PERCENT) / 100, locale)}</span>
    </button>
  );
}

export interface ContextRingProps {
  thread: Thread;
  side?: 'top' | 'bottom';
  /** Where the box opens it by a command of its own. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** A thread that is only looked at shows the same figure and never changes the size. */
  readOnly?: boolean;
}

export function ContextRing({ thread, side = 'bottom', open, onOpenChange, readOnly = false }: ContextRingProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const runs = useRunsWith(thread, true);
  const state = runs.states.find((one): one is ContextState => one.kind === 'context');
  // A size is chosen among the model's own window, the largest that is open and a few between.
  const sizes = state === undefined ? [] : contextChoices({ least: state.least, own: state.window, largest: state.largest });
  const choosable = state !== undefined && sizes.length > 1;
  const ring = ringOf(thread.usage);
  // Where the harness gave the tokens but no window, the tokens stand alone.
  const used = ring === null ? usedAlone(thread.usage) : ring.used;
  // A context size that the model's managers opened and this thread cannot use is shown off, with why.
  const off = state === undefined ? withheldOf(runs.parameters, 'context') : undefined;
  const offModel = runs.parameters?.model.options.find((option) => option.model_name === runs.parameters?.model.value);
  const offRows = off === undefined || offModel === undefined ? [] : sizeRows(offModel.parameters, locale);
  const writable = state !== undefined && !readOnly && state.writable && state.providerFixed !== true && runs.ready && !runs.choosing && holds(thread.access, 'thread.manage');
  const window = state?.force ?? thread.usage?.context_window ?? offModel?.parameters.context_window ?? null;
  const size = window !== null && window > 0 ? formatContext(window, locale) : offRows.find((row) => row.checked === true)?.name ?? '';
  const context = used === null ? size === '' ? t('contextUsage.ring.notReported') : t('contextUsage.ring.labelWindow', { window: size })
    : ring === null ? t('contextUsage.ring.labelUsed', { used: formatTokens(used, locale) }) : ringWords(ring, t, locale);
  const reportedModel = contextModelName(thread);
  const words = reportedModel === null ? context : t('contextUsage.ring.modelLabel', { model: modelDisplayName(reportedModel, t), context });
  const trigger = used === null
    ? <Tooltip content={words}><Button size="sm" variant="ghost" aria-label={size === '' ? t('contextUsage.ring.open', { label: words }) : t('composer.chip.value', { what: t('composer.model.context.title'), value: size })} icon={<Gauge aria-hidden />} data-context-ring="unmeasured" className={cx('gap-1 px-1 text-xs tabular-nums text-text-muted', off !== undefined && 'opacity-60')}>{size === '' ? t('composer.model.context.title') : size}</Button></Tooltip>
    : <Tooltip content={words}><RingButton ring={ring} used={used} words={words} /></Tooltip>;
  return (
    <Popover
      label={t('contextUsage.panel.title')}
      side={side}
      align="start"
      className="max-h-(--radix-popover-content-available-height) overflow-y-auto"
      {...(open === undefined ? {} : { open })}
      {...(onOpenChange === undefined ? {} : { onOpenChange })}
      trigger={trigger}
    >
      <div className="flex flex-col gap-3">
        {choosable && <ContextChoice state={state} writable={writable} chooser={runs.chooser(state.key)} onChoose={(chosen) => runs.choose(state, chosen)} />}
        {off !== undefined && offRows.length > 0 && <PickList label={t('composer.model.context.title')} groups={[{ id: 'sizes', label: null, rows: offRows }]} above={<WithheldNote thread={thread} entry={off} />} onChoose={() => undefined} />}
        {runs.chooseError !== null && <ErrorAlert error={runs.chooseError} />}
        {(used !== null || !choosable) && <ContextPanel thread={thread} />}
      </div>
    </Popover>
  );
}
