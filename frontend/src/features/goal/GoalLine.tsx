/** The goal of a thread as one quiet line above the composer: its status, its condition, what it has used, and Pause.
 *
 * "Working", "Waiting for you", "Paused", "Met" or "Stopped" is the chip, in words and an icon; the condition is
 * shortened with its whole on hover and on focus; the figures are tabular and keep their room, and a state that waits
 * or stopped says why in their place. Pause or Resume, and ⋯ for Edit and Clear, are for whoever may drive the thread;
 * others see the same line with the acts disabled and the reason. On a phone the chip and the condition show, and a
 * tap opens the rest.
 */
import { useMutation } from '@tanstack/react-query';
import { Check, Ellipsis, LoaderCircle, OctagonAlert, Pause, Play, ShieldQuestionMark, Target } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { clearGoal, updateGoal } from '@/api/goals';
import { openGoalEditor } from '@/features/goal/editor';
import { chipOf, elapsedMs, figures, isLive, limitStillReached, reasonWords, waitingWords, type GoalChip } from '@/features/goal/model';
import type { Thread } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { useRunningClock } from '@/lib/time';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { StatusChip, type StatusTone } from '@/ui/StateChip';
import { Tooltip } from '@/ui/Tooltip';

const CHIPS: Record<GoalChip, { tone: StatusTone; icon: ReactNode }> = {
  working: { tone: 'info', icon: <LoaderCircle className="animate-spin motion-reduce:animate-none" aria-hidden /> },
  waiting: { tone: 'warning', icon: <ShieldQuestionMark aria-hidden /> },
  paused: { tone: 'idle', icon: <Pause aria-hidden /> },
  met: { tone: 'success', icon: <Check aria-hidden /> },
  stopped: { tone: 'warning', icon: <OctagonAlert aria-hidden /> },
};

export function GoalLine({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const read = useThreadReadRef(thread.id);
  const [open, setOpen] = useState(false);
  const goal = thread.goal ?? null;
  const clock = useRunningClock(goal?.status === 'active');
  const change = useMutation({ mutationFn: (action: 'pause' | 'resume') => updateGoal(read, { action }) });
  const clear = useMutation({ mutationFn: () => clearGoal(read) });
  if (goal === null) return null;
  const may = holds(thread.access, 'thread.interact') && !thread.archived;
  const chip = chipOf(goal);
  const look = CHIPS[chip];
  const used = figures(goal, t, locale, goal.status === 'active' ? elapsedMs(goal, clock) : goal.used.elapsed_secs * 1000);
  const why = chip === 'waiting' && goal.status === 'active'
    ? waitingWords(goal, t)
    : goal.status === 'paused' || goal.status === 'stopped' ? reasonWords(goal, t, locale) : null;
  const how = goal.status === 'met' ? t(goal.last_report?.checked_by === 'helper' ? 'goals.line.byHelper' : 'goals.line.byAgent') : null;
  const checking = goal.checking ? t('goals.checking') : null;
  const condition = t('goals.line.condition', { condition: goal.condition });
  const side = why ?? [used.join(' · '), checking, how].filter((part): part is string => part !== null && part !== '').join(' · ');
  const quiet = (goal.status === 'active' && why === null) || goal.status === 'met';
  const reachedFirst = limitStillReached(goal);
  const driveOnly = may ? undefined : t('goals.act.driveOnly');

  const toggle = isLive(goal) || goal.status === 'stopped' ? (
    goal.status === 'active'
      ? <IconButton key="pause" size="sm" label={t('goals.act.pause')} icon={<Pause aria-hidden />} pending={change.isPending && change.variables === 'pause'}
        blocked={!may} reason={driveOnly} onClick={() => change.mutate('pause')} />
      : <IconButton key="resume" size="sm" label={t('goals.act.resume')} icon={<Play aria-hidden />} pending={change.isPending && change.variables === 'resume'}
        blocked={!may || reachedFirst} reason={driveOnly ?? (reachedFirst ? t('goals.act.raiseFirst') : undefined)} onClick={() => change.mutate('resume')} />
  ) : null;
  const entries: MenuEntry[] = [
    ...(goal.status === 'met' ? [] : [{ id: 'goal-edit', label: t('goals.act.edit'), disabled: driveOnly ?? false, onSelect: () => openGoalEditor(thread.id) } as MenuEntry]),
    { id: 'goal-clear', label: t('goals.act.clear'), disabled: driveOnly ?? false, onSelect: () => clear.mutate() },
  ];
  const acts = (
    <span className="flex shrink-0 items-center gap-1">
      {toggle}
      <Menu label={t('goals.act.more')} align="end" entries={entries} trigger={<IconButton size="sm" label={t('goals.act.more')} icon={<Ellipsis aria-hidden />} />} />
    </span>
  );
  const error = change.error ?? clear.error;
  return (
    <ContextMenu entries={entries} label={t('goals.act.more')}>
      <section aria-label={t('goals.line.label')} data-goal-line={chip} className="@container flex min-w-0 flex-col gap-1 rounded-md px-3 py-1 text-sm text-text-muted">
      <div className="flex min-h-8 min-w-0 items-center gap-2">
        <Tooltip content={[goal.status === 'paused' && goal.detail !== null ? goal.detail : used.join(' · '), isLive(goal) && !goal.agent_reports ? t('goals.line.noReports') : ''].filter((part) => part !== '').join('. ')}>
          <span className="inline-flex shrink-0"><StatusChip tone={look.tone} icon={look.icon}>{t(`goals.status.${chip}`)}</StatusChip></span>
        </Tooltip>
        <button type="button" aria-expanded={open} onClick={() => setOpen((held) => !held)}
          className="focus-inset flex min-w-0 flex-1 cursor-default items-center text-left @xl:hidden">
          <span className={cx('min-w-0', open ? 'wrap-anywhere' : 'truncate')}>{condition}</span>
        </button>
        <span className="hidden min-w-0 flex-1 items-center gap-1.5 @xl:flex">
          <Target aria-hidden className="shrink-0" />
          <span className="min-w-0 truncate" data-overflow-text={goal.objective === goal.condition ? goal.condition : `${goal.condition}\n${goal.objective}`}>{condition}</span>
        </span>
        <span className={cx('hidden min-w-0 max-w-[45%] shrink truncate tabular-nums @xl:inline', !quiet && 'text-warning')} data-goal-figures>{side}</span>
        <span className="hidden @xl:flex">{acts}</span>
      </div>
      {open && (
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 @xl:hidden">
          <span className={cx('min-w-0 flex-1 wrap-anywhere tabular-nums', !quiet && 'text-warning')}>{side}</span>
          {acts}
        </div>
      )}
      {error !== null && <ErrorAlert error={error} message={t('goals.act.failed')} />}
      </section>
    </ContextMenu>
  );
}
