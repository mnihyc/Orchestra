/** Shared StateChip component (status); primitives are wrapped here.
 *
 * What something is doing, as an icon in the colour of its state and its name in words. The colour
 * never stands alone: a row that has no room for the word shows the icon, whose name a screen reader
 * says and the pointer reads.
 */
import { Archive, Check, ChevronDown, type Circle, LoaderCircle, MessageSquareText, CircleCheck, CirclePause, CircleSlash, Clock3, OctagonAlert, ShieldQuestionMark, Unplug } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { ReactNode, Ref } from 'react';

import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

/** The states work can be in. */
export type WorkState =
  | 'running'
  | 'starting'
  | 'waiting_approval'
  | 'blocked'
  | 'failed'
  | 'queued'
  | 'held'
  | 'finished'
  | 'idle'
  | 'done'
  | 'archived'
  | 'offline';

const STATES: Record<WorkState, { tone: string; Icon: typeof Circle; spins?: true }> = {
  running: { tone: 'text-info', Icon: LoaderCircle, spins: true },
  starting: { tone: 'text-info', Icon: LoaderCircle, spins: true },
  waiting_approval: { tone: 'text-warning', Icon: ShieldQuestionMark },
  blocked: { tone: 'text-warning', Icon: CircleSlash },
  failed: { tone: 'text-danger', Icon: OctagonAlert },
  queued: { tone: 'text-text-muted', Icon: Clock3 },
  held: { tone: 'text-text-muted', Icon: CirclePause },
  finished: { tone: 'text-success', Icon: CircleCheck },
  idle: { tone: 'text-text-subtle', Icon: MessageSquareText },
  done: { tone: 'text-success', Icon: Check },
  archived: { tone: 'text-text-disabled', Icon: Archive },
  offline: { tone: 'text-text-disabled', Icon: Unplug },
};

/** Unknown recorded states retain their text instead of inventing a status glyph. */
export function isWorkState(value: string): value is WorkState { return Object.hasOwn(STATES, value); }

export interface StateIconProps {
  state: WorkState;
  /** Other words for the state, where the caller knows more ("Waiting for 2 approvals"). */
  label?: string | undefined;
  className?: string | undefined;
  /** The SVG ref for a heading or tab glyph. */
  glyphRef?: Ref<SVGSVGElement> | undefined;
}

/** The icon of a state alone, named for screen readers and under the pointer. */
export function StateIcon({ state, label, className, glyphRef }: StateIconProps) {
  const { t } = useTranslation();
  const { tone, Icon, spins } = STATES[state];
  const name = label ?? t(`state.${state}`);
  return (
    <Tooltip content={name}>
      <span role="img" aria-label={name} data-work-state={state} className={cx('inline-flex shrink-0', tone, className)}>
        <Icon ref={glyphRef} className={cx(spins && 'motion-safe:animate-spin')} aria-hidden />
      </span>
    </Tooltip>
  );
}

export interface StateChipProps {
  state: WorkState;
  /** Other words for the state. */
  label?: string | undefined;
  /** The chip is a menu trigger; its caller supplies the button and the menu. */
  menu?: boolean;
  className?: string;
}

export type StatusTone = 'neutral' | 'accent' | 'info' | 'success' | 'warning' | 'danger' | 'idle' | 'off';

const TONES: Record<StatusTone, string> = {
  neutral: 'text-text-muted', accent: 'text-accent-text', info: 'text-info', success: 'text-success',
  warning: 'text-warning', danger: 'text-danger', idle: 'text-text-subtle', off: 'text-text-muted',
};

/** The one status shape for work states, named resource states and word badges; counters stay separate. */
export function StatusChip({ children, icon, tone = 'neutral', className }: { children: ReactNode; icon?: ReactNode; tone?: StatusTone; className?: string | undefined }) {
  return <span data-status-chip className={cx('inline-flex min-h-6 max-w-full shrink-0 items-center gap-1.5 rounded-full border border-border bg-surface px-2 py-0.5 text-sm leading-4 text-text', className)}>
    <span aria-hidden className={cx('inline-flex shrink-0 items-center justify-center [&_svg]:size-3.5', TONES[tone])}>
      {icon ?? <span className="size-1.5 rounded-full bg-current" />}
    </span>
    <span className="min-w-0 wrap-anywhere">{children}</span>
  </span>;
}

/** A state as its icon and its name. */
export function StateChip({ state, label, menu = false, className }: StateChipProps) {
  const { t } = useTranslation();
  const { tone, Icon, spins } = STATES[state];
  return (
    <StatusChip className={className} icon={<Icon className={cx(tone, spins && 'motion-safe:animate-spin')} aria-hidden />}>
      {menu ? <span className="inline-flex items-center gap-1">{label ?? t(`state.${state}`)}<ChevronDown className="size-3.5 text-text-subtle" aria-hidden /></span> : label ?? t(`state.${state}`)}
    </StatusChip>
  );
}
