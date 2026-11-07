/** Compact named work and its effective wake setting, using the header's existing control sizes. */
import { Bot, SquareTerminal } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { BackgroundCommand, BackgroundWakeStatus, BackgroundWork, ThreadBackground } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { backgroundName, backgroundNotice } from './backgroundWake';

export function BackgroundWorkLine({ status, canManage, mayEnd, onSetting, onEnd, children }: {
  status: BackgroundWakeStatus | null;
  canManage: boolean;
  mayEnd: boolean;
  onSetting: () => void;
  onEnd: (task: BackgroundWork) => void;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  if (status === null || status.tasks.length === 0 && status.held_completions === 0 && status.waiting_running === 0 && !status.refresh_deferred) return children ?? null;
  const notice = backgroundNotice(status, t);
  const setting = t(status.effective ? 'backgroundWake.settingOn' : 'backgroundWake.settingOff');
  const tasks = children === undefined ? status.tasks : status.tasks.filter((task) => task.kind === 'command');
  return <div role="group" aria-label={t('backgroundWake.work')} className="flex min-w-0 max-w-full flex-wrap items-center gap-x-2 gap-y-1" data-background-work>
    {tasks.length > 0 && <ul className="flex min-w-0 max-w-full flex-wrap items-center gap-1">
      {tasks.map((task) => {
        const name = backgroundName(task, t);
        const Icon = task.kind === 'subagent' ? Bot : SquareTerminal;
        return <li key={task.id} className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-1 rounded-md border border-border px-2 text-xs" data-background-task={task.id} data-background-state={task.state}>
          <Icon className="size-3 shrink-0" aria-hidden />
          <span className="min-w-0 wrap-anywhere whitespace-normal">{name}</span>
          <span className="shrink-0 text-text-muted">{t(`backgroundWake.states.${task.state}`)}</span>
          {task.state === 'running' && (task.can_end && mayEnd ? <Button size="sm" variant="ghost" aria-label={t('backgroundWake.endNamed', { name })} onClick={() => onEnd(task)}>{t('running.end')}</Button>
            : mayEnd && !task.can_end ? <span className="min-w-0 wrap-anywhere text-text-subtle">{t('backgroundWake.cannotEnd')}</span> : null)}
        </li>;
      })}
    </ul>}
    {children}
    <span className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-x-2 gap-y-1 text-xs text-text-muted" data-background-wake-notice>
      {notice !== null && <span>{notice}</span>}
      {canManage ? <Button size="sm" variant="ghost" onClick={onSetting}>{setting}</Button> : <span>{setting}</span>}
      {status.setting !== status.effective && <span>{t(status.setting ? 'backgroundWake.pendingOn' : 'backgroundWake.pendingOff')}</span>}
      {status.refresh_deferred && <span>{t('backgroundWake.deferred')}</span>}
    </span>
    {status.held_completions > 0 && <span className="min-w-0 wrap-anywhere text-xs text-text-muted" data-background-held>{t('backgroundWake.held', { count: status.held_completions })}</span>}
    {status.waiting_running > 0 && <span className="min-w-0 wrap-anywhere text-xs text-text-muted">{t(status.waiting_author === null ? 'backgroundWake.waitingUnknownAuthor' : 'backgroundWake.waitingAuthor', { name: status.waiting_author?.display_name, count: status.waiting_running })}</span>}
  </div>;
}

/** Older native observations remain visible without inventing a recorded owner or a wake promise. */
export function NativeBackgroundCommands({ roster, represented, mayEnd, onEnd }: {
  roster: ThreadBackground | null;
  represented: readonly BackgroundWork[];
  mayEnd: boolean;
  onEnd: (command: BackgroundCommand) => void;
}) {
  const { t } = useTranslation();
  if (roster?.state !== 'live') return null;
  const ends = new Set(represented.flatMap((task) => typeof task.end_id === 'string' ? [task.end_id] : []));
  const recordedSources = new Map<string, number>(), nativeSources = new Map<string, number>();
  const source = (item: string, turn: string) => JSON.stringify([item, turn]);
  for (const task of represented) if (task.state === 'running' && task.end_id === null) {
    const key = source(task.item_id, task.turn_id);
    recordedSources.set(key, (recordedSources.get(key) ?? 0) + 1);
  }
  for (const command of roster.items) if (typeof command.item_id === 'string' && typeof command.turn_id === 'string') {
    const key = source(command.item_id, command.turn_id);
    nativeSources.set(key, (nativeSources.get(key) ?? 0) + 1);
  }
  const commands = roster.items.filter((command) => {
    if (ends.has(command.id)) return false;
    if (typeof command.item_id !== 'string' || typeof command.turn_id !== 'string') return true;
    const key = source(command.item_id, command.turn_id);
    // A reused source item is not an exact task/run identity; keep ambiguous native work visible.
    return recordedSources.get(key) !== 1 || nativeSources.get(key) !== 1;
  });
  if (commands.length === 0) return null;
  return <ul className="flex min-w-0 max-w-full flex-wrap items-center gap-1" aria-label={t('backgroundWake.work')}>
    {commands.map((command) => {
      const name = command.command ?? t('backgroundWake.unnamedCommand');
      return <li key={command.id} className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-1 rounded-md border border-border px-2 text-xs" data-native-background-command={command.id}>
        <SquareTerminal className="size-3 shrink-0" aria-hidden /><span className="min-w-0 wrap-anywhere whitespace-normal">{name}</span>
        <span className="text-text-muted">{t('backgroundWake.states.running')}</span>
        {roster.can_end && mayEnd && <Button size="sm" variant="ghost" aria-label={t('backgroundWake.endNamed', { name })} onClick={() => onEnd(command)}>{t('running.end')}</Button>}
      </li>;
    })}
  </ul>;
}
