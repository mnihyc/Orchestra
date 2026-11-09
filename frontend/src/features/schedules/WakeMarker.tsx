/** A client-chosen hint cannot label a turn: current native metadata and the actual run must agree. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { inputScheduleQuery, inputScheduleRunsQuery } from '@/api/inputSchedules';
import { hintedSchedule, isAgentSchedule, runWokeInput } from './facts';

export function ScheduleWakeMarker({ thread, input, turn, clientId, enabled }: { thread: string; input: string | null; turn: string | null; clientId: string | null | undefined; enabled: boolean }) {
  const id = hintedSchedule(clientId);
  return !enabled || id === null || input === null || turn === null ? null : <WakeProof key={JSON.stringify([thread, input, turn, id])} thread={thread} input={input} turn={turn} id={id} />;
}

function WakeProof({ thread, input, turn, id }: { thread: string; input: string; turn: string; id: string }) {
  const { t } = useTranslation();
  useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  const schedule = useQuery(inputScheduleQuery(id));
  const row = schedule.data;
  const eligible = !schedule.isError && row !== undefined && row.id === id && isAgentSchedule(row, thread);
  const latest = eligible && row.last_run !== null && runWokeInput(row, row.last_run, thread, input, turn);
  const history = useInfiniteQuery({ ...inputScheduleRunsQuery(id), enabled: eligible && !latest });
  const verified = latest || eligible && !history.isError && (history.data?.pages.some((page) => page.items.some((run) => runWokeInput(row, run, thread, input, turn))) ?? false);
  const { fetchNextPage, hasNextPage, isFetching, isError } = history;
  // Only a rendered input with an actual lookup hint walks history. The native page bound limits
  // each read; max_runs is not a client cap. Stop as soon as this input is proved, refused or absent.
  useEffect(() => {
    if (eligible && !verified && hasNextPage && !isFetching && !isError) void fetchNextPage({ cancelRefetch: false });
  }, [eligible, verified, hasNextPage, isFetching, isError, fetchNextPage]);
  return verified ? <span data-schedule-wake className="text-sm text-text-muted">{t('agentSchedules.woken')}</span> : null;
}
