/** A plan's counts retain the service's units and one bounded list of visible dependent names. */
import { useTranslation } from 'react-i18next';
import { useState } from 'react';
import { Input } from '@/ui/Input';
import type { ActivityGroup,DependentGroup,ResourceFacts,ResourceImpact } from '@/generated/api-types';
import { DependentName } from './Storage';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DetailCard } from '@/ui/DetailCard';
import { RelativeTime } from '@/ui/RelativeTime';

export function ImpactNames({ group, more = false }: { group: Pick<ActivityGroup, 'visible' | 'hidden'>; more?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [search, setSearch] = useState('');
  const needle = search.trim().toLocaleLowerCase(locale);
  const visible = group.visible.filter((row) => row.name.toLocaleLowerCase(locale).includes(needle));
  return <div className="flex max-h-[min(22rem,var(--radix-popper-available-height,22rem))] w-[18rem] max-w-full flex-col gap-2 p-3">
    {group.visible.length > 10 && <Input label={t('resourceImpact.findNames')} value={search} onChange={(event) => setSearch(event.target.value)} />}
    <ul className="flex min-h-0 min-w-0 flex-col gap-1 overflow-y-auto">{visible.map((row, index) => <li key={`${row.object?.id ?? 'hidden'}:${index}`} className="flex min-h-(--control-sm) min-w-0 items-center"><DependentName row={row} /></li>)}</ul>
    {group.hidden > 0 && <p className="text-sm text-text-muted">{more ? t('resourceImpact.moreThreads', { count: group.hidden, number: formatNumber(group.hidden, locale) }) : t('storage.hidden', { number: formatNumber(group.hidden, locale) })}</p>}
  </div>;
}

/** These are the service's complete consequence groups, already restricted to this act by its producer. */
export function MountEffects({ facts }: { facts: ResourceFacts }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return <div className="flex flex-wrap items-center gap-2">{facts.dependents.filter((group) => group.count > 0).map((group) => {
    const label = group.relation === 'attachment' ? t('resourceDetails.mount.detachNextStart', { count: group.count, number: formatNumber(group.count, locale) }) : t('resourceDetails.mount.withdrawNow', { count: group.count, number: formatNumber(group.count, locale) });
    return <DetailCard key={group.relation} label={label} content={() => <ImpactNames group={group} />}><span className="text-sm text-text-muted underline decoration-dotted underline-offset-4">{label}</span></DetailCard>;
  })}</div>;
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
