/** A plan's counts retain the service's units and one bounded list of visible dependent names. */
import { useTranslation } from 'react-i18next';
import type { ActivityGroup,DependentGroup,ResourceImpact } from '@/generated/api-types';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DetailCard } from '@/ui/DetailCard';
import { RelativeTime } from '@/ui/RelativeTime';

export function ImpactNames({group,more=false}:{group:Pick<ActivityGroup,'visible'|'hidden'>;more?:boolean}) {
  const { t }=useTranslation();
  const { locale }=useFormatContext();
  return <div className="max-h-[min(22rem,var(--radix-popper-available-height,22rem))] w-[18rem] max-w-full overflow-y-auto p-3"><ul className="flex min-w-0 flex-col gap-1">{group.visible.map((row,index)=><li key={`${row.object?.id ?? 'hidden'}:${index}`} className="flex min-h-(--control-sm) min-w-0 items-center">{row.object?.kind==='thread' ? <KnownNodeChip node={{id:row.object.id,kind:'thread',name:row.name,redacted:false}} /> : <span className="truncate" title={row.name}>{row.name}</span>}</li>)}</ul>{group.hidden>0 && <p className="mt-2 text-sm text-text-muted">{more?t('resourceImpact.moreThreads',{count:group.hidden,number:formatNumber(group.hidden,locale)}):t('storage.hidden',{number:formatNumber(group.hidden,locale)})}</p>}</div>;
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
  return <div className="flex min-w-0 flex-col gap-2">{kind==='enable'?<><ActivityCount group={impact.activity.waiting} kind="waiting" /><ActivityCount group={impact.activity.stopped} kind="stopped" /></>:<ActivityCount group={impact.activity.running} kind="running" />}<BindingCount group={impact.facts.dependents.find((group)=>group.relation==='binding')} /></div>;
}
