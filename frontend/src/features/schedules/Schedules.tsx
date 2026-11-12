/** One schedule presentation and cancellation act for the timeline and the thread's list. */
import { useIsMutating, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { CalendarClock } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { cancelInputSchedule, inputScheduleQuery, inputScheduleRunsQuery, pendingAgentSchedulesQuery, pendingSchedulesRefused, topicSchedulesQuery, type InputSchedule, type ThreadPendingAgentSchedules } from '@/api/inputSchedules';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { ThreadPanel } from '@/features/inspector/ThreadPanel';
import { SettingsEditor } from '@/features/settings/Editor';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { ViewProps } from '@/features/workbench/views';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { formatDateTime, formatExact } from '@/lib/time';
import { threadReadOnly, useThreadPart } from '@/realtime/stores/thread';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { HumanScheduleActions, ScheduleHistory } from './HumanSchedules';
import { belongsToThread, isAgentSchedule } from './facts';

export const AGENT_SCHEDULE_SETTINGS = ['threads.agent_schedules', 'threads.agent_schedule_hours', 'threads.agent_schedule_runs'] as const;
const WEEKDAYS = ['agentSchedules.weekdays.1', 'agentSchedules.weekdays.2', 'agentSchedules.weekdays.3', 'agentSchedules.weekdays.4', 'agentSchedules.weekdays.5', 'agentSchedules.weekdays.6', 'agentSchedules.weekdays.7'] as const;

function ScheduleTime({ at }: { at: string | null }) {
  const format = useFormatContext();
  return at === null ? <span>–</span> : <Tooltip content={formatExact(at, format)}><time dateTime={at}>{formatDateTime(at, format)}</time></Tooltip>;
}

function ScheduleOutcome({ row }: { row: InputSchedule }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  return <div className="flex min-w-0 flex-col items-start gap-1"><Badge>{t(`agentSchedules.states.${row.state}`)}</Badge>
    {row.last_run !== null && <div className="flex min-w-0 flex-col gap-1 text-sm text-text-muted"><span>{t(`agentSchedules.outcomes.${row.last_run.state}`)}</span><ScheduleTime at={row.last_run.started_at ?? row.last_run.scheduled_for} />{row.last_run.reason !== null && <span>{describe(row.last_run.reason).message}</span>}</div>}
  </div>;
}

export function ScheduleCancel({ schedule, topic, thread, allowed, reason }: { schedule: Pick<InputSchedule, 'id' | 'state'>; topic: string; thread?: string; allowed: boolean; reason: string | undefined }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  const mutationKey = ['input-schedule-cancel', schedule.id, reader] as const;
  const pending = useIsMutating({ mutationKey }) > 0;
  const cancel = useMutation({ mutationKey,
    mutationFn: (asked: { id: string; reader: ReturnType<typeof libraryReader> }) => {
      if (!sameLibraryReader(asked.reader)) return Promise.reject(new DOMException('The schedule reader changed', 'AbortError'));
      return cancelInputSchedule(asked.id);
    }, onSuccess: (_reply, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      if (thread !== undefined) {
        const key = pendingAgentSchedulesQuery(thread, topic).queryKey;
        cache.setQueryData<ThreadPendingAgentSchedules>(key, (before) => before === undefined ? undefined
          : { ...before, items: before.items.filter((item) => item.id !== asked.id) });
        void cache.invalidateQueries({ queryKey: key, exact: true });
      }
      void cache.invalidateQueries({ queryKey: topicSchedulesQuery(topic).queryKey, exact: true });
      void cache.invalidateQueries({ queryKey: inputScheduleQuery(asked.id).queryKey, exact: true });
      void cache.invalidateQueries({ queryKey: inputScheduleRunsQuery(asked.id).queryKey, exact: true });
    } });
  const current = cancel.variables?.id === schedule.id && sameLibraryReader(cancel.variables.reader);
  const future = schedule.state === 'active' || schedule.state === 'paused';
  return <div className="flex min-w-0 flex-wrap items-center gap-2">
    {current && cancel.isSuccess ? <span role="status" className="text-sm text-text-muted">{t('agentSchedules.cancelled')}</span>
      : future && <Button size="sm" variant="ghost" pending={pending} blocked={!allowed} reason={reason} onClick={() => { if (allowed && !pending) cancel.mutate({ id: schedule.id, reader: libraryReader() }); }}>{t('common.cancel')}</Button>}
    {current && cancel.error !== null && <ErrorAlert error={cancel.error} />}
  </div>;
}

function useControl(thread: Thread, readOnly: boolean) {
  const { t } = useTranslation();
  const user = useStore(client.session, (state) => state.userId);
  return (author?: string) => {
    const allowed = !readOnly && (holds(thread.access, 'thread.interact') || author !== undefined && author === user);
    return { allowed, reason: allowed ? undefined : t(readOnly ? 'composer.row.readOnly' : 'agentSchedules.controlNeeded') };
  };
}

function useSchedules(topic: string, enabled: boolean) {
  // Retire an already cached projection on reader changes, as well as fencing in-flight responses.
  useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return useQuery({ ...topicSchedulesQuery(topic), enabled });
}

/** The opening owns the current-thread subset; topic events request a fresh native projection. */
export function PendingAgentSchedules({ thread, readOnly = false }: { thread: Thread; readOnly?: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  const query = useQuery({ ...pendingAgentSchedulesQuery(thread.id, thread.topic_id),
    enabled: !readOnly && !pendingSchedulesRefused(cache, thread.id, thread.topic_id) });
  const control = useControl(thread, readOnly);
  if (readOnly) return null;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === undefined) return <SkeletonRows rows={1} />;
  const pending = query.data.items;
  if (pending.length === 0) return null;
  return <ol aria-label={t('agentSchedules.pending')} className="mx-auto flex w-full max-w-(--reading-width) flex-col gap-1 px-4 py-2" data-agent-schedule-lines>
    {pending.map((schedule) => <li key={schedule.id} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
      <CalendarClock aria-hidden className="size-4 shrink-0" /><span>{t('agentSchedules.continueAt')}</span><ScheduleTime at={schedule.next_run_at} />
      <span aria-hidden>·</span><span className="min-w-0 flex-1 truncate" title={schedule.name}>{schedule.name}</span>
      <ScheduleCancel schedule={schedule} topic={thread.topic_id} thread={thread.id} {...control()} />
    </li>)}
  </ol>;
}

/** People's and agent-authored entries share their actual destination, state and ordinary cancellation route. */
export function ThreadSchedules({ thread, readOnly = false }: { thread: Thread; readOnly?: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const query = useSchedules(thread.topic_id, !readOnly);
  const control = useControl(thread, readOnly);
  if (readOnly) return <p role="status" className="p-3 text-sm text-text-muted">{t('composer.row.readOnly')}</p>;
  const rows = (query.data ?? []).filter((row) => belongsToThread(row, thread.id));
  return <div className="flex min-w-0 flex-col gap-4 p-3">
    <section className="min-w-0">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2"><h2 className="text-sm font-medium">{t('agentSchedules.title')}</h2>
        <Button size="sm" variant="ghost" pending={query.isRefetching} onClick={() => void query.refetch()}>{t('common.refresh')}</Button></div>
      {query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.data === undefined ? <SkeletonRows rows={4} />
        : rows.length === 0 ? <EmptyState compact title={t('agentSchedules.empty')} /> : <Table label={t('agentSchedules.title')} rows={rows} rowKey={(row) => row.id}
          columns={[
            { id: 'name', header: t('agentSchedules.name'), grow: true, cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1"><span className="truncate" title={row.name}>{row.name}</span>
              <div className="flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted">{isAgentSchedule(row, thread.id) && <span>{t('agentSchedules.agentFor')}</span>}<UserChip user={row.author} /></div>
              {row.timing.kind === 'cron' && <span className="text-sm text-text-muted"><code>{row.timing.expression}</code> · {row.timing.zone}</span>}
              {row.timing.kind === 'repeat' && <span className="text-sm text-text-muted">{t(`agentSchedules.repeat.${row.timing.frequency}`)}{row.timing.frequency === 'weekly' && ` ${new Intl.ListFormat(format.locale).format(row.timing.weekdays.map((day) => { const key = WEEKDAYS[day - 1]; return key === undefined ? '–' : t(key); }))}`} · {row.timing.time} · {row.timing.zone}</span>}
              {(row.agent !== null && row.agent !== undefined) && isAgentSchedule(row, thread.id) && <div className="flex flex-wrap items-baseline gap-1 text-sm text-text-muted"><span>{t('agentSchedules.runs', { done: formatNumber(row.agent.runs, format.locale), total: formatNumber(row.agent.max_runs, format.locale) })}</span><span aria-hidden>·</span><span>{t('agentSchedules.until')}</span><ScheduleTime at={row.agent.until} /></div>}
              {!shown.has('next') && <div className="flex min-w-0 flex-wrap items-baseline gap-1 text-sm text-text-muted"><span>{t('agentSchedules.next')}</span><ScheduleTime at={row.next_run_at} /></div>}
              {!shown.has('state') && <ScheduleOutcome row={row} />}<ScheduleHistory row={row} />
            </div> },
            { id: 'next', header: t('agentSchedules.next'), from: 'sm', cell: (row) => <ScheduleTime at={row.next_run_at} /> },
            { id: 'state', header: t('agentSchedules.state'), from: 'md', cell: (row) => <ScheduleOutcome row={row} /> },
            { id: 'cancel', header: t('common.cancel'), hideHeader: true, align: 'end', cell: (row) => <HumanScheduleActions row={row} topic={thread.topic_id} thread={thread} controller={control(row.author.id).allowed} /> },
          ]} />}
    </section>
    <SettingsEditor target={{ kind: 'node', id: thread.id }} scope="thread" place={{ kind: 'thread', name: thread.name, owner: thread.owner, access: thread.access }} only={AGENT_SCHEDULE_SETTINGS} compact />
  </div>;
}

function CurrentSchedules({ thread }: { thread: Thread }) {
  const readOnly = useThreadPart(thread.id, threadReadOnly);
  return <ThreadSchedules thread={thread} readOnly={readOnly} />;
}
export function SchedulesView({ tab }: ViewProps) {
  const shown = useViewShown();
  return !shown ? null : <ThreadPanel threadId={tab.of?.thread} icon={<CalendarClock aria-hidden />}>{(thread) => <CurrentSchedules thread={thread} />}</ThreadPanel>;
}
