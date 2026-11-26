/** The same schedule time and cancellation act, shared without loading a management screen. */
import { useIsMutating, useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { cancelInputSchedule, inputScheduleQuery, inputScheduleRunsQuery, pendingAgentSchedulesQuery, topicSchedulesQuery, type InputSchedule, type ThreadPendingAgentSchedules } from '@/api/inputSchedules';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { formatDateTime, formatExact } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Tooltip } from '@/ui/Tooltip';

export function ScheduleTime({ at }: { at: string | null }) {
  const format = useFormatContext();
  return at === null ? <span>–</span> : <Tooltip content={formatExact(at, format)}><time dateTime={at}>{formatDateTime(at, format)}</time></Tooltip>;
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

export function useScheduleControl(thread: Thread, readOnly: boolean) {
  const { t } = useTranslation();
  const user = useStore(client.session, (state) => state.userId);
  return (author?: string) => {
    const allowed = !readOnly && (holds(thread.access, 'thread.interact') || author !== undefined && author === user);
    return { allowed, reason: allowed ? undefined : t(readOnly ? 'composer.row.readOnly' : 'agentSchedules.controlNeeded') };
  };
}
