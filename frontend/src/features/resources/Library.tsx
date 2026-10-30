/** A resource's giving and reported use share one shape on its row and page. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { api, ok } from '@/api/client';
import { RESOURCE_PAGE_SIZE } from '@/api/resources';
import type { ResourceFacts, ResourceUseKind, LibraryTarget, LibraryEntry } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DetailCard } from '@/ui/DetailCard';
import { PeopleList } from '@/ui/PeopleList';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { Hint } from '@/features/help/Hint';
import { RelativeTime } from '@/ui/RelativeTime';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { resourceSubjectOf } from '@/features/resource-facts/facts';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';

export type LibraryObject=LibraryTarget['object'];
type LibraryKind='provider'|'agent'|'environment'|'host'|'template'|'mcp_server'|'mount'|'skill';
const applicable = new Set<string>(['provider','agent','environment','host','template','mcp_server','mount','skill']);
export const hasLibrary = (kind: string): kind is LibraryKind => applicable.has(kind);
export const libraryQuery = (targets: readonly LibraryTarget[]) => ({
  queryKey: ['resource-library', targets] as const,
  queryFn: ({ signal }: {signal: AbortSignal}): Promise<LibraryEntry[]> => ok(api.POST('/resources/library', {body: {targets: [...targets]}, signal})),
});

function Names({ group }: { group: ResourceFacts['dependents'][number] }) {
  const { t }=useTranslation();
  const {locale}=useFormatContext();
  const [offset,setOffset]=useState(0);
  const page=group.visible.slice(offset,offset+RESOURCE_PAGE_SIZE);
  return <div className="max-h-[min(20rem,var(--radix-popper-available-height,20rem))] w-72 max-w-full overflow-y-auto p-3"><ul className="flex min-w-0 flex-col gap-1">{page.map((row,index) => {
    const object=row.object;
    const named=object===null?null:{...object,name:row.name};
    const resource=named === null ? null : resourceSubjectOf(named,row.name);
    return <li key={`${object?.id ?? ''}:${index}`} className="flex min-h-(--control-sm) min-w-0">{resource !== null ? <ResourceChip subject={resource} hint={false} /> : object !== null && ['thread','org','project','topic'].includes(object.kind) ? <KnownNodeChip node={{id:object.id,kind:object.kind as 'thread' | 'org' | 'project' | 'topic',name:row.name,redacted:false}} /> : <span className="truncate" title={row.name}>{row.name}</span>}</li>;
  })}</ul>{group.hidden>0 && <p className="mt-2 text-sm text-text-muted">{t('library.more',{number:formatNumber(group.hidden,locale)})}</p>}{group.visible.length>RESOURCE_PAGE_SIZE&&<div className="mt-2 flex justify-between gap-2"><Button size="sm" disabled={offset===0} onClick={()=>setOffset(0)}>{t('documentFolders.beginning')}</Button><Button size="sm" disabled={offset+RESOURCE_PAGE_SIZE>=group.visible.length} onClick={()=>setOffset(offset+RESOURCE_PAGE_SIZE)}>{t('documentFolders.next')}</Button></div>}</div>;
}

const unit = (kind: ResourceUseKind) => ({turn:'turns',mcp_call:'calls',skill_invocation:'invocations',environment_made:'environments',provider_made:'providers'} as const)[kind];
function givenUnit(kind:string,relation:string,use:ResourceUseKind|undefined) {
  if(relation==='explicit_giving')return 'places';
  if(relation==='binding'||kind==='skill')return 'threads';
  if(kind==='host')return 'units';
  return use==='provider_made'?'providers':'environments';
}

export function LibraryCounts({facts,threads,full=false,version=null,given=true}:{facts:ResourceFacts;threads?:number|null;full?:boolean;version?:string|null;given?:boolean}) {
  const {t}=useTranslation(); const {locale}=useFormatContext();
  const figure=(n:number|null|undefined)=>n===null||n===undefined?'–':formatNumber(n,locale,full?undefined:{notation:'compact',maximumFractionDigits:1});
  const uses=facts.uses;
  const kind=facts.subject.kind==='resource'?facts.subject.object.kind:'';
  if (!hasLibrary(kind)) return null;
  const giving=facts.dependents.filter((group)=>['binding','explicit_giving','attachment'].includes(group.relation));
  const relations=kind==='skill'||kind==='mcp_server'?['explicit_giving','attachment']:kind==='provider'||kind==='agent'||kind==='environment'?['binding']:['attachment'];
  return <div data-library-counts className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1 text-sm text-text-muted tabular-nums">
    {given&&relations.map((relation)=>{
      const group=giving.find((g)=>g.relation===relation);
      const unitKey=givenUnit(kind,relation,uses?.kind);
      const label=t('library.given',{number:figure(facts.dependents_complete ? group?.count??0 : null),unit:t(`library.units.${unitKey}`,{count:group?.count??0})});
      return group===undefined||!facts.dependents_complete?<span key={relation}>{label}</span>:<DetailCard key={relation} label={label} content={()=><Names group={group}/>}><span className="underline decoration-dotted underline-offset-4">{label}</span></DetailCard>;
    })}
    {uses!==null && <DetailCard label={t('library.usedTitle')} content={()=><div className="flex w-72 max-w-full flex-col gap-2 p-3"><LibraryUse facts={facts} threads={threads} version={version}/></div>}><span className="underline decoration-dotted underline-offset-4">{t('library.used',{number:figure(uses.all_time),unit:t(`library.units.${unit(uses.kind)}`,{count:uses.all_time??0})})}</span></DetailCard>}
    {full && uses!==null && <LibraryUse facts={facts} threads={threads} version={version}/>}
  </div>;
}

function LibraryUse({facts,threads,version}:{facts:ResourceFacts;threads?:number|null;version:string|null}) {
  const {t}=useTranslation();const {locale}=useFormatContext();const uses=facts.uses;
  const n=(value:number|null)=>value===null?'–':formatNumber(value,locale);
  if(uses===null)return null;
  return <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1 text-sm text-text-muted">
    <span>{t('library.allTime',{number:n(uses.all_time)})}</span><span>{t('library.recent',{number:n(uses.last_30_days)})}</span>
    {facts.subject.kind==='resource' && facts.subject.object.kind==='provider'&&<span>{t('library.threadCount',{number:n(threads??null),count:threads??0})}</span>}
    <UsePeople facts={facts} count={uses.people} version={version} label={t('library.people',{number:n(uses.people),count:uses.people??0})}/>
    <UsePeople facts={facts} count={uses.people_30_days} version={version} recent label={t('library.recentPeople',{number:n(uses.people_30_days),count:uses.people_30_days??0})}/>
    {uses.last_used_at!==null&&<span>{t('library.lastUsed')} <RelativeTime at={uses.last_used_at}/></span>}
    {uses.coverage.state==='unrecorded'?<span>{t('library.unrecorded')}</span>:uses.coverage.state==='partial'?<span>{t('library.partial')} <RelativeTime at={uses.coverage.since}/></span>:null}
  </div>;
}

function UsePeople({facts,count,label,recent=false,version}:{facts:ResourceFacts;count:number|null;label:string;recent?:boolean;version:string|null}) {
  if(count===null || facts.subject.kind!=='resource')return <span>{label}</span>;
  const object=facts.subject.object;
  return <DetailCard label={label} content={()=><UsePeopleList object={object} recent={recent} version={version}/>}><span className="underline decoration-dotted underline-offset-4">{label}</span></DetailCard>;
}
function UsePeopleList({object,recent,version}:{object:LibraryObject;recent:boolean;version:string|null}) {
  const {t}=useTranslation();
  const [cursor,setCursor]=useState<string|null>(null);
  const query=useQuery({queryKey:['resource-library-people',object,recent,version,cursor],
    // Hover-to-pin shares the same admitted read; a cursor replaces its page rather than appending history.
    queryFn:()=>ok(api.POST('/resources/library/people',{body:{target:{object,version_id:version},cursor,recent}}))});
  if(query.isPending)return <div className="w-72 p-3"><SkeletonRows rows={3}/></div>;
  if(query.isError)return <ErrorState compact error={query.error} onRetry={()=>void query.refetch()}/>;
  if(query.data.items.length===0&&query.data.hidden===0)return <EmptyState compact title={t('library.noPeople')}/>;
  return <div className="p-3"><PeopleList users={query.data.items} hidden={query.data.hidden}>
    {(cursor!==null||query.data.next_cursor!==null)&&<div className="mt-2 flex justify-between gap-2"><Button size="sm" disabled={cursor===null} onClick={()=>setCursor(null)}>{t('documentFolders.beginning')}</Button><Button size="sm" disabled={query.data.next_cursor===null} onClick={()=>setCursor(query.data.next_cursor)}>{t('documentFolders.next')}</Button></div>}
  </PeopleList></div>;
}

export function ResourceLibrary({object,version=null}:{object:LibraryObject;version?:string|null}) {
  const query=useQuery({...libraryQuery([{object,version_id:version}]),retry:false});
  if(query.isPending)return <SkeletonRows rows={1}/>;
  if(query.isError)return <ErrorState compact error={query.error} onRetry={()=>void query.refetch()}/>;
  const entry=query.data[0];
  return entry===undefined?null:<div className="flex min-w-0 items-start gap-2"><LibraryCounts facts={entry.facts} threads={entry.threads} version={version} given={version===null} full/><Hint id="resource-library" popover/></div>;
}
