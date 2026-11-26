/** Pending lines use the thread opening without fetching schedule management. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { CalendarClock } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { pendingAgentSchedulesQuery, pendingSchedulesRefused } from '@/api/inputSchedules';
import type { Thread } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { ScheduleCancel, ScheduleTime, useScheduleControl } from './ScheduleControls';

/** The opening owns the current-thread subset; topic events request a fresh native projection. */
export function PendingAgentSchedules({ thread, readOnly = false }: { thread: Thread; readOnly?: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  const query = useQuery({ ...pendingAgentSchedulesQuery(thread.id, thread.topic_id),
    enabled: !readOnly && !pendingSchedulesRefused(cache, thread.id, thread.topic_id) });
  const control = useScheduleControl(thread, readOnly);
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
