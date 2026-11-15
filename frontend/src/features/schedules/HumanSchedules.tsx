/** Human definitions are author-private; state controls keep native revisions and queued work intact. */
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { meQuery } from '@/api/auth';
import { inputScheduleQuery, inputScheduleRunsQuery, topicSchedulesQuery, updateInputSchedule, type InputSchedule } from '@/api/inputSchedules';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { InputScheduleUpdateBody, Thread, Topic } from '@/generated/api-types';
import { Link } from '@tanstack/react-router';
import { useActKey } from '@/lib/forms';
import { formatDateTime } from '@/lib/time';
import { useFormatContext } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { useErrorText } from '@/ui/errorText';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';
import { Section } from '@/shell/Page';
import { ScheduleEditor, ScheduleModes } from './Editor';
import { ScheduleCancel } from './Schedules';

export function HumanScheduleActions({ row, topic, controller, thread }: { row: InputSchedule; topic: string; controller: boolean; thread?: Thread }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const me = useQuery(meQuery);
  const readerKey = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  const [editing, setEditing] = useState(false);
  const key = useActKey();
  const author = me.data?.id === row.author.id;
  const human = row.kind !== 'agent' && row.timing.kind !== 'cron';
  const future = row.state === 'active' || row.state === 'paused';
  const change = useMutation({ mutationKey: ['schedule-state', row.id, readerKey], mutationFn: (asked: { body: InputScheduleUpdateBody; reader: ReturnType<typeof libraryReader> }) => {
    if (!sameLibraryReader(asked.reader)) return Promise.reject(new DOMException('The schedule reader changed.', 'AbortError'));
    return updateInputSchedule(row.id, asked.body, key(asked.body));
  }, onSuccess: (result, asked) => {
    if (!sameLibraryReader(asked.reader)) return;
    if (!('projection' in result)) cache.setQueryData(inputScheduleQuery(row.id).queryKey, result);
  }, onError: (error) => {
    if (error instanceof ApiError && error.is('conflict.stale_revision')) { void cache.invalidateQueries({ queryKey: inputScheduleQuery(row.id).queryKey, exact: true }); void cache.invalidateQueries({ queryKey: topicSchedulesQuery(topic).queryKey, exact: true }); }
  } });
  const current = change.variables !== undefined && sameLibraryReader(change.variables.reader);
  const changed = current && change.isSuccess && change.variables.body.revision === row.revision;
  const setState = (state: 'active' | 'paused') => {
    const body: InputScheduleUpdateBody = { revision: row.revision, name: null, target: null, content: null, timing: null, state };
    change.mutate({ body, reader: libraryReader() });
  };
  return <div className="flex min-w-0 flex-wrap items-center gap-2">
    {human && author && row.definition !== null && <Button size="sm" variant="ghost" onClick={() => setEditing(true)}>{t('sendLater.edit')}</Button>}
    {human && future && (author || controller) && <Button size="sm" variant="ghost" pending={change.isPending} disabled={changed || row.state === 'paused' && !author} reason={row.state === 'paused' && !author ? t('sendLater.authorOnly') : undefined} onClick={() => setState(row.state === 'active' ? 'paused' : 'active')}>{t(row.state === 'active' ? 'sendLater.pause' : 'sendLater.resume')}</Button>}
    {changed && <span role="status" className="text-sm text-text-muted">{t(change.variables.body.state === 'paused' ? 'agentSchedules.states.paused' : 'agentSchedules.states.active')}</span>}
    <ScheduleCancel schedule={row} topic={topic} thread={thread?.id} allowed={author || controller} reason={author || controller ? undefined : t('agentSchedules.controlNeeded')} />
    {current && change.isError && <ErrorAlert error={change.error} />}
    {editing && row.definition !== null && <ScheduleEditor key={`${row.id}:${readerKey}`} topic={topic} thread={thread} schedule={row} onClose={() => setEditing(false)} />}
  </div>;
}
export function ScheduleHistory({ row }: { row: InputSchedule }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const [open, setOpen] = useState(false);
  const format = useFormatContext();
  const query = useInfiniteQuery({ ...inputScheduleRunsQuery(row.id), enabled: open });
  return <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}><summary className="cursor-pointer text-sm text-text-muted">{t('sendLater.history')}</summary>
    {open && (query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.data === undefined ? <SkeletonRows rows={2} /> : <ol className="flex max-h-[50dvh] flex-col gap-2 overflow-auto py-2 text-sm">
      {query.data.pages.flatMap((page) => page.items).map((run) => <li key={run.id} className="flex flex-wrap items-baseline gap-2"><span>{t(`agentSchedules.outcomes.${run.state}`)}</span><time dateTime={run.scheduled_for ?? run.created_at}>{formatDateTime(run.scheduled_for ?? run.created_at, { ...format, timeZone: row.timing.zone })}</time>
        {run.thread !== null && <Link to="/thread/$threadId" params={{ threadId: run.thread.id }} hash={run.input_id === null ? undefined : `input-${run.input_id}`}>{run.thread.name ?? t('access.place.hidden')}</Link>}{run.reason !== null && <span>{describe(run.reason).message}</span>}
      </li>)}
    </ol>)}
    {open && query.hasNextPage && <Button size="sm" pending={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>{t('operation.more')}</Button>}
  </details>;
}
export function TopicSchedules({ topic }: { topic: Topic }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  const query = useQuery(topicSchedulesQuery(topic.id));
  const controller = holds(topic.access, 'topic.interact');
  return <Section id="scheduled-inputs" title={t('sendLater.list')}>
    {query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.data === undefined ? <SkeletonRows rows={3} /> : query.data.length === 0 ? <p className="text-sm text-text-muted">{t('sendLater.empty')}</p> : <Table label={t('sendLater.list')} rows={query.data} rowKey={(row) => row.id} columns={[
      { id: 'name', header: t('agentSchedules.name'), grow: true, cell: (row) => <div className="flex min-w-0 flex-col gap-1"><span className="wrap-anywhere">{row.name}</span><UserChip user={row.author} /><span className="text-sm text-text-muted">{row.destination.new_thread ? t('sendLater.newThread') : row.destination.thread?.name ?? t('access.place.hidden')}</span><ScheduleHistory row={row} /></div> },
      { id: 'next', header: t('agentSchedules.next'), from: 'sm', cell: (row) => row.next_run_at === null ? '–' : formatDateTime(row.next_run_at, { ...format, timeZone: row.timing.zone }) },
      { id: 'state', header: t('agentSchedules.state'), from: 'sm', cell: (row) => <div className="flex flex-col gap-1"><span>{t(`agentSchedules.states.${row.state}`)}</span><ScheduleModes plan={row.plan} delivery={row.delivery} /></div> },
      { id: 'actions', header: t('sendLater.actions'), hideHeader: true, cell: (row) => <HumanScheduleActions row={row} topic={topic.id} controller={controller} /> },
    ]} />}
  </Section>;
}
