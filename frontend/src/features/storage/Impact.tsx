/** A plan's counts retain the service's units and one bounded list of visible dependent names. */
import { useTranslation } from 'react-i18next';
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import type { ActivityGroup,DependentGroup,FactTarget,ResourceApplicationGroup,ResourceImpact } from '@/generated/api-types';
import { resourceImpactQuery } from '@/api/resource-impact';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { resourceSubjectOf } from '@/features/resource-facts/facts';
import { openChange } from '@/features/change/target';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DetailCard } from '@/ui/DetailCard';
import { RelativeTime } from '@/ui/RelativeTime';
import { Input } from '@/ui/Input';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function ImpactNames({group,more=false}:{group:Pick<ActivityGroup,'visible'|'hidden'>;more?:boolean}) {
  const { t }=useTranslation();
  const { locale }=useFormatContext();
  const [search, setSearch] = useState('');
  const wanted = search.trim().toLocaleLowerCase();
  const rows = group.visible.filter((row) => wanted === '' || row.name.toLocaleLowerCase().includes(wanted));
  return <div className="flex max-h-[min(22rem,var(--radix-popper-available-height,22rem))] w-[18rem] max-w-full flex-col gap-2 p-3">
    {(group.visible.length > 10 || search !== '') && <Input label={t('common.search')} hideLabel value={search} onChange={(event) => setSearch(event.target.value)} />}
    <ul className="flex min-h-0 min-w-0 flex-col gap-1 overflow-y-auto">{rows.map((row,index) => {
      const subject = row.object === null ? null : resourceSubjectOf(row.object, row.name);
      return <li key={`${row.object?.id ?? 'hidden'}:${index}`} className="flex min-h-(--control-sm) min-w-0 items-center">{row.object?.kind === 'thread'
        ? <KnownNodeChip node={{id:row.object.id,kind:'thread',name:row.name,redacted:false}} />
        : subject !== null ? <ResourceChip subject={subject} hint={false}>{row.name}</ResourceChip>
        : <span className="truncate" title={row.name}>{row.name}</span>}</li>;
    })}</ul>
    {rows.length === 0 && wanted !== '' && <span className="text-sm text-text-muted">{t('resourceImpact.noNames')}</span>}
    {group.hidden > 0 && <p className="text-sm text-text-muted">{more?t('resourceImpact.moreThreads',{count:group.hidden,number:formatNumber(group.hidden,locale)}):t('storage.hidden',{number:formatNumber(group.hidden,locale)})}</p>}
  </div>;
}

export function ActivityCount({group,kind}:{group:ActivityGroup;kind:'running'|'waiting'|'stopped'}) {
  const { t }=useTranslation();
  const { locale }=useFormatContext();
  if(group.count===0)return null;
  const label=t(`resourceImpact.${kind}`,{count:group.count,number:formatNumber(group.count,locale),threads:t('resourceImpact.threadCount',{count:group.threads,number:formatNumber(group.threads,locale)})});
  return <span className="flex min-w-0 flex-wrap items-center gap-x-1 text-sm text-text-muted tabular-nums"><DetailCard label={label} content={() => <ImpactNames group={group} more={kind==='waiting'} />}><span className="underline decoration-dotted underline-offset-4">{label}</span></DetailCard>{kind==='waiting' && group.since!==null && <span>· {t('resourceImpact.since')} <RelativeTime at={group.since} /></span>}</span>;
}

export function BindingCount({group}:{group:DependentGroup|undefined}) {
  const { t }=useTranslation();const { locale }=useFormatContext();
  if(group===undefined || group.count===0)return null;
  const label=t('resourceImpact.bound',{count:group.count,number:formatNumber(group.count,locale)});
  return <DetailCard label={label} content={() => <ImpactNames group={group} />}><span className="text-sm text-text-muted underline decoration-dotted underline-offset-4">{label}</span></DetailCard>;
}

export function ImpactCounts({impact,kind}:{impact:ResourceImpact;kind:'disable'|'enable'|'delete'}) {
  return <div className="flex min-w-0 flex-col gap-2">{kind==='enable'?<><ActivityCount group={impact.activity.waiting} kind="waiting" /><ActivityCount group={impact.activity.stopped} kind="stopped" /></>:<ActivityCount group={impact.activity.running} kind="running" />}<BindingCount group={impact.facts.dependents.find((group)=>group.relation==='binding')} /><ApplicationCounts impact={impact} kind={kind} /></div>;
}

function ApplicationCount({group,complete}:{group:ResourceApplicationGroup;complete:boolean}) {
  const {t}=useTranslation(); const {locale}=useFormatContext();
  const value=formatNumber(group.count,locale), number=complete?value:`≥${value}`;
  const values={count:group.count,number};
  const label=group.when==='now'&&group.object_kind==='thread'?t('resourceImpact.application.nowThreads',values)
    :group.when==='before_next_turn'&&group.object_kind==='thread'?t('resourceImpact.application.nextTurnThreads',values)
    :group.when==='updated'&&group.object_kind==='environment'?t('resourceImpact.application.updatedEnvironments',values)
    :group.when==='updated'&&group.object_kind==='provider'?t('resourceImpact.application.updatedProviders',values):null;
  if(label===null||group.count===0)return null;
  const ids=group.apply_targets.filter((target)=>target.kind===group.object_kind).map((target)=>target.id);
  const apply=()=>{
    if(ids.length===0)return;
    if(group.object_kind==='environment')openChange({kind:'environments',ids},{kind:'update'});
    else if(group.object_kind==='provider')openChange({kind:'providers',ids},{kind:'update'});
  };
  return <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted tabular-nums">
    <DetailCard label={label} content={()=><ImpactNames group={group}/>}><span className="underline decoration-dotted underline-offset-4">{label}</span></DetailCard>
    {group.apply!==null&&<Button size="sm" variant="ghost" blocked={ids.length===0} reason={ids.length===0?t('resources.act.manageRequired'):undefined} onClick={apply}>{t('change.env.menu.update')}</Button>}
  </span>;
}

export function ApplicationCounts({impact,kind='change'}:{impact:ResourceImpact;kind?:'change'|'disable'|'enable'|'delete'}) {
  const {t}=useTranslation();
  return <div className="flex min-w-0 flex-col gap-1">
    {!impact.application_complete&&<span role="status" className="text-sm text-text-muted">{t('resourceImpact.applicationIncomplete')}</span>}
    {impact.applications.filter((group)=>group.when!=='now'||kind==='disable'||kind==='delete').map((group)=><ApplicationCount key={`${group.when}:${group.object_kind}`} group={group} complete={impact.application_complete}/>)}
  </div>;
}

/** The same read gates editing and draws its current source; a refused cached answer stays hidden. */
export function useApplicationImpact(target:FactTarget,active:boolean) {
  const shown=useViewShown();
  const read=useQuery({...resourceImpactQuery(target),enabled:active&&shown});
  return {read,ready:!active||shown&&read.isSuccess&&!read.isError};
}

export function ApplicationPreview({read,kind='change'}:{read:ReturnType<typeof useApplicationImpact>['read'];kind?:'change'|'disable'|'enable'|'delete'}) {
  if(read.isError)return <ErrorState compact error={read.error} onRetry={()=>void read.refetch()}/>;
  if(read.data===undefined)return <SkeletonRows read={read} rows={2}/>;
  return <ApplicationCounts impact={read.data} kind={kind}/>;
}
