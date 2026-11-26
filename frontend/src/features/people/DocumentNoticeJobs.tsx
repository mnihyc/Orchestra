/** Active notice work keeps its source search and Stop controller above the bounded rows. */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client as apiClient } from '@/api/client';
import { documentNoticeJobsQuery } from '@/api/documentNoticeJobs';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { cancelJob } from '@/api/exports';
import { JobActivityLine } from '@/features/change/Progress';
import type { Job } from '@/generated/api-types';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { SkeletonRows } from '@/ui/Skeleton';

export function DocumentNoticeJobs({document}:{document:string}) {
  const search=useCollectionSearch(`document-notice-jobs:${document}`);
  return <NoticeJobsForReader key={search.scope} document={document} search={search}/>;
}
function NoticeJobsForReader({document,search}:{document:string;search:ReturnType<typeof useCollectionSearch>}) {
  const {t}=useTranslation();
  const cache=useQueryClient();
  const reader=useStore(apiClient.session,(state)=>state.readerVersion??0);
  const signedIn=useStore(apiClient.session,(state)=>state.status==='signed_in');
  const options=documentNoticeJobsQuery(document,reader,search.q);
  const jobs=useInfiniteQuery({...options,enabled:signedIn});
  const stop=useMutation({mutationFn:({job}:{job:Job;reader:ReturnType<typeof libraryReader>})=>cancelJob(job.id),
    onSuccess:(_result,variables)=>{if(sameLibraryReader(variables.reader))return jobs.refetch();}});
  const found=jobs.data?.pages.flatMap((page)=>page.items)??[];
  const held=stop.isPending || stop.isError?stop.variables.job:undefined;
  const rows=held!==undefined && !found.some((row)=>row.id===held.id)?[held,...found]:found;
  const later=jobs.data?.pageParams[0]!==undefined && jobs.data.pageParams[0]!==null;
  if(rows.length===0 && search.text==='' && !later && !jobs.isError)return null;
  return <div className="flex min-w-0 flex-col gap-2">
    <CollectionSearch name={t('people.quick.notificationWork')} {...search}/>
    {later && <Button size="sm" onClick={()=>void cache.resetQueries({queryKey:options.queryKey,exact:true})}>{t('people.quick.latestNotices')}</Button>}
    {jobs.isPending?<SkeletonRows read={jobs} rows={2}/>:jobs.isLoadingError?<ErrorState compact error={jobs.error} onRetry={()=>void jobs.refetch()}/>:rows.length===0?<EmptyState compact title={t('ui.collection.none')}/>:null}
    <ScrollRegion kind="table" label={t('people.quick.notificationWork')} className="max-h-64 overflow-y-auto"><ul aria-label={t('people.quick.notificationWork')} className="flex flex-col gap-2">{rows.map((job)=><li key={job.id} data-notice-job={job.id} className="flex flex-col gap-1"><div className="flex flex-wrap items-center justify-between gap-2"><JobActivityLine job={job}/><Button size="sm" disabled={stop.isPending} pending={stop.isPending && stop.variables.job.id===job.id} onClick={()=>stop.mutate({job,reader:libraryReader()})}>{t('people.quick.stopNotices')}</Button></div></li>)}</ul></ScrollRegion>
    {stop.error!==null && <ErrorAlert error={stop.error}/>}
    {jobs.isRefetchError && <ErrorAlert error={jobs.error} action={<Button size="sm" onClick={()=>void jobs.refetch()}>{t('common.tryAgain')}</Button>}/>}
    <Pagination hasMore={jobs.hasNextPage} loading={jobs.isFetchingNextPage} error={jobs.isFetchNextPageError?jobs.error:undefined} onMore={()=>void jobs.fetchNextPage()}/>
  </div>;
}
