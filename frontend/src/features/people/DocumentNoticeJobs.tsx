/** A discussion's background notices stay visible, with bounded pages and the common job activity line. */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client as apiClient } from '@/api/client';
import { documentNoticeJobsQuery } from '@/api/documentNoticeJobs';
import { cancelJob } from '@/api/exports';
import { JobActivityLine } from '@/features/change/Progress';
import type { Job } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';

function NoticeJob({ job, onStopped }: { job: Job; onStopped: () => Promise<unknown> }) {
  const { t } = useTranslation();
  const stop = useMutation({ mutationFn: () => cancelJob(job.id), onSuccess: onStopped });
  return <li data-notice-job={job.id} className="flex flex-col gap-1"><div className="flex flex-wrap items-center justify-between gap-2"><JobActivityLine job={job} /><Button size="sm" pending={stop.isPending} onClick={() => stop.mutate()}>{t('people.quick.stopNotices')}</Button></div>{stop.error !== null && <ErrorAlert error={stop.error} />}</li>;
}

export function DocumentNoticeJobs({ document }: { document: string }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const reader = useStore(apiClient.session, (state) => state.readerVersion ?? 0);
  const signedIn = useStore(apiClient.session, (state) => state.status === 'signed_in');
  const options = documentNoticeJobsQuery(document, reader);
  const jobs = useInfiniteQuery({ ...options, enabled: signedIn });
  if (jobs.isPending) return null;
  if (jobs.isLoadingError) return <ErrorState error={jobs.error} onRetry={() => void jobs.refetch()} compact />;
  const rows = jobs.data.pages.flatMap((page) => page.items);
  const later = jobs.data.pageParams[0] !== null;
  if (rows.length === 0 && !later) return null;
  return <div className="flex flex-col gap-2">{later && <Button size="sm" onClick={() => void client.resetQueries({ queryKey: options.queryKey, exact: true })}>{t('people.quick.latestNotices')}</Button>}<ul aria-label={t('people.quick.notificationWork')} className="flex flex-col gap-2">{rows.map((job) => <NoticeJob key={job.id} job={job} onStopped={() => jobs.refetch()} />)}</ul>{jobs.isRefetchError && <ErrorAlert error={jobs.error} action={<Button size="sm" onClick={() => void jobs.refetch()}>{t('common.tryAgain')}</Button>} />}<Pagination hasMore={jobs.hasNextPage} loading={jobs.isFetchingNextPage} error={jobs.isFetchNextPageError ? jobs.error : undefined} onMore={() => void jobs.fetchNextPage()} /></div>;
}
