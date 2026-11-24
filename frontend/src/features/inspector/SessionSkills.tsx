/** Accepted session choices and omissions, loaded only inside their shared fact opening. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { sessionSkillsQuery } from '@/api/sessionSkills';
import type { SessionSkillEntry, SessionSkillsSummary, SkillSkip, Thread } from '@/generated/api-types';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure, FactDisclosureBody } from '@/ui/FactDisclosure';
import { FilterBar } from '@/ui/FilterBar';
import { Input } from '@/ui/Input';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import type { ThreadReadTarget } from '@/realtime/threadScope';

const reasons:Record<SkillSkip,string>={missing:'sessionSkills.reasons.missing',disabled:'sessionSkills.reasons.disabled',deleted:'sessionSkills.reasons.deleted',
  no_version:'sessionSkills.reasons.noVersion',version_not_found:'sessionSkills.reasons.versionNotFound',not_usable:'sessionSkills.reasons.notUsable',name_taken:'sessionSkills.reasons.nameTaken'};

function Details({threadId,read,summary}:{threadId:string;read:ThreadReadTarget;summary:SessionSkillsSummary}) {
  const {t}=useTranslation();const {locale}=useFormatContext();
  const [search,setSearch]=useState('');
  // The Environment inspector is a current runtime view, independent of a stored conversation selection.
  const query=useInfiniteQuery(sessionSkillsQuery(read,summary));
  const {hasNextPage,isFetching,isError,fetchNextPage}=query;
  const first=query.data?.pages[0];
  const entries=query.data?.pages.flatMap((page)=>page.entries.items)??[];
  useEffect(()=>{if(search.trim()!==''&&hasNextPage&&!isFetching&&!isError)void fetchNextPage();},[search,hasNextPage,isFetching,isError,fetchNextPage]);
  const words=(entry:SessionSkillEntry)=>[entry.skill?.name,entry.contributor?.display_name,
    entry.source.kind==='place'?entry.source.setting?.node?.name:t('sessionSkills.personal'),
    entry.decision.kind==='skipped'?t(reasons[entry.decision.reason]):entry.decision.version?.number.toString()].filter(Boolean).join(' ').toLocaleLowerCase();
  const filtered=entries.map((entry,index)=>({entry,index})).filter(({entry})=>words(entry).includes(search.trim().toLocaleLowerCase()));
  if(query.isError)return <ErrorState compact error={query.error} onRetry={()=>void query.refetch()}/>;
  return <div className="flex min-w-0 flex-col gap-2">
    <FactDisclosureBody groups={[{key:'accepted',kind:'recorded_outcome',facts:[
      {key:'applied',label:t('sessionSkills.applied'),value:<RelativeTime at={summary.applied_at}/>},
      {key:'next',label:t('sessionSkills.nextSelection'),value:first?.next_session_changed===undefined||first.next_session_changed===null?undefined:t(first.next_session_changed?'sessionSkills.nextChanged':'sessionSkills.nextSame')},
    ]}]}/>
    {!summary.supported&&<span className="text-sm text-text-muted">{t('sessionSkills.unsupported')}</span>}
    <FilterBar search={<Input label={t('sessionSkills.search')} hideLabel type="search" value={search} onChange={(event)=>setSearch(event.target.value)} autoComplete="off"/>}/>
    {query.isPending||query.isFetching?<SkeletonRows rows={2}/>:<>
      <FactDisclosureBody groups={[{key:'choices',facts:filtered.map(({entry,index})=>({key:String(index),
        label:entry.skill===null?t('sessionSkills.notShared'):<ResourceChip subject={{kind:'skill',skill:entry.skill}} hint={false}/>,
        value:<span className="inline-flex min-w-0 flex-wrap items-baseline gap-1">
          <span>{entry.decision.kind==='selected'?entry.decision.version===null?t('sessionSkills.notShared'):t('sessionSkills.version',{number:formatNumber(entry.decision.version.number,locale)}):t('sessionSkills.skipped',{reason:t(reasons[entry.decision.reason])})}</span>
          <span>·</span>{entry.source.kind==='personal'?<span>{t('sessionSkills.personal')}</span>:entry.source.setting===null?<span>{t('sessionSkills.notShared')}</span>:<SourceSettingLink inherited source={entry.source.setting} def={{key:'agents.skills',class:'parameter'}} target={{kind:'node',id:threadId}} fallback={t('sessionSkills.notShared')}/>}
          <span>·</span>{entry.contributor===null?<span>{t('sessionSkills.notShared')}</span>:<UserChip user={entry.contributor}/>}
        </span>,
      }))}]}/>
      {filtered.length===0&&!query.hasNextPage&&<span className="text-sm text-text-muted">{t(entries.length===0?'sessionSkills.empty':'sessionSkills.noMatch')}</span>}
    </>}
    {query.hasNextPage&&<Button size="sm" pending={query.isFetchingNextPage} onClick={()=>void query.fetchNextPage()}>{t('sessionSkills.more')}</Button>}
  </div>;
}

export function SessionSkills({thread,read}:{thread:Thread;read?:ThreadReadTarget}) {
  const {t}=useTranslation();const {locale}=useFormatContext();
  const summary=thread.binding?.session_skills;
  return <div className="flex min-w-0 flex-wrap items-center gap-1 text-sm">
    <span>{summary?t('sessionSkills.closed',{selected:formatNumber(summary.selected,locale),skipped:formatNumber(summary.skipped,locale)}):t('sessionSkills.title')}</span>
    {summary?<FactDisclosure fragment={!summary.supported?t('sessionSkills.unsupported'):undefined} groups={[]}>
      <Details key={summary.session_revision} threadId={thread.id} read={read??thread.id} summary={summary}/>
    </FactDisclosure>:<FactDisclosure groups={[{key:'unreported',kind:'recorded_outcome',facts:[{key:'selection',label:t('sessionSkills.title')}]}]}/>}
  </div>;
}
