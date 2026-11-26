/** The choice inspector reads saved pages under the current reader and starts no work. */
import { useStore } from 'zustand';
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { poolChoicesQuery, poolChoiceDetailsQuery } from '@/api/pools';
import type { PoolChoicesQuery, Turn } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';

const beginning:PoolChoicesQuery={before_created_at:null,before_id:null};
function choiceReason(t: TFunction, reason: string) {
  return t([`pools.reasons.${reason}`, `calls.details.${reason}`, 'pools.unavailable']);
}

export function PoolHistory({id}:{id:string}) {
  const reader=useStore(client.session,(state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  return <History key={`${id}:${reader}`} id={id} reader={reader} />;
}
function History({id,reader}:{id:string;reader:string}) {
  const {t}=useTranslation();
  const [before,setBefore]=useState(beginning);
  const [text,setText]=useState('');
  const [choice,setChoice]=useState<string|null>(null);
  const query=useQuery(poolChoicesQuery(id,before,reader));
  const next=query.isError ? null : query.data?.next ?? null;
  const name=(label:string|null,reason:string|null)=>label ?? t(reason==='member_hidden' ? 'pools.hidden':'pools.member');
  const rows=query.isError ? [] : (query.data?.items ?? []).filter((row)=>
    [name(row.label,row.reason),t(`pools.choiceState.${row.state}`),row.policy===null?'':t(`pools.policy.${row.policy.type}`),row.reason===null?'':choiceReason(t,row.reason)]
      .some((value)=>value.toLocaleLowerCase().includes(text.trim().toLocaleLowerCase())));
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex gap-2"><Input label={t('pools.searchChoices')} hideLabel value={text} onChange={(event)=>setText(event.target.value)} />
      <Button size="sm" disabled={query.isFetching} onClick={()=>void query.refetch()}>{t('pools.readAgain')}</Button></div>
    {query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError ? <ErrorState compact error={query.error} onRetry={()=>void query.refetch()} /> :
      <ul className="max-h-[60vh] overflow-auto divide-y divide-border" aria-label={t('pools.history')}>
        {rows.length===0 && <li className="py-2 text-sm text-text-muted">{t('pools.noChoices')}</li>}
        {rows.map((row)=><li key={row.id} className="flex min-w-0 flex-col gap-2 py-2">
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1 text-sm">
            <RelativeTime at={row.created_at} compact /><span>{name(row.label,row.reason)}</span>
            <span className="text-text-muted">{row.policy===null ? t(`pools.choiceState.${row.state}`) : t(`pools.policy.${row.policy.type}`)}</span>
            {row.reason!==null && row.reason!=='member_hidden' && <span className="text-text-muted">{choiceReason(t,row.reason)}</span>}
            <Button size="sm" aria-label={t('pools.choiceInspect',{name:name(row.label,row.reason)})} aria-expanded={choice===row.id} onClick={()=>setChoice(choice===row.id?null:row.id)}>{t('common.details')}</Button>
          </div>
          {choice===row.id && <Details key={row.id} id={id} choice={row.id} reader={reader} comparesCosts={row.policy?.type === 'least_cost'} />}
        </li>)}
      </ul>}
    <div className="flex justify-between gap-2"><Button size="sm" disabled={query.isFetching || before.before_id===null} onClick={()=>{setBefore(beginning);setChoice(null);}}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={query.isFetching || next===null} onClick={()=>{if(next!==null){setBefore(next);setChoice(null);}}}>{t('documentFolders.next')}</Button></div>
  </div>;
}
const rateSourceLabel = {
  provider: 'usage.rates.from.provider',
  rate_card: 'usage.records.fromCard',
  catalogue: 'usage.records.fromCatalogue',
  reported: 'usageRequest.vendorTotal',
} as const;

/** Expand the same reader-scoped comparison as the pool history, from the turn that used it. */
export function TurnPoolReceipt({ receipt }: { receipt: NonNullable<Turn['pool_choice']> }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const choices = [receipt.pool_name, receipt.member ?? t('pools.hidden')];
  if (receipt.policy) choices.push(t(`pools.policy.${receipt.policy.type}`));
  return <div className="flex min-w-0 flex-col items-start gap-1">
    <button type="button" className="text-left hover:underline" aria-expanded={open} onClick={() => setOpen(!open)}>
      {t('timeline.footer.options', { choices: choices.join(' · ') })}
    </button>
    {open && <Details key={`${reader}:${receipt.choice_id}`} id={receipt.pool_id} choice={receipt.choice_id} reader={reader} comparesCosts={receipt.policy?.type === 'least_cost'} />}
  </div>;
}

function Details({id,choice,reader,comparesCosts}:{id:string;choice:string;reader:string;comparesCosts:boolean}) {
  const {t}=useTranslation();
  const [after,setAfter]=useState<number|null>(null);
  const [text,setText]=useState('');
  const query=useQuery(poolChoiceDetailsQuery(id,choice,after,reader));
  if(query.isPending) return <SkeletonRows read={query} rows={2} />;
  if(query.isError) return <ErrorState compact error={query.error} onRetry={()=>void query.refetch()} />;
  if(query.data.expired_at!==null) return <span className="text-sm text-text-muted">{t('pools.choiceExpired')}</span>;
  const rows=query.data.items.filter((row)=>(row.label ?? t('pools.hidden')).toLocaleLowerCase().includes(text.trim().toLocaleLowerCase()));
  const next=query.data.next;
  return <div className="flex min-w-0 flex-col gap-2 border-l border-border pl-3">
    <Input label={t('pools.searchMembers')} hideLabel value={text} onChange={(event)=>setText(event.target.value)} />
    <ul className="max-h-[40vh] overflow-auto text-sm">
      {rows.length===0 && <li>{t('pools.noChoiceDetails')}</li>}
      {rows.map((row)=><li key={row.ordinal} className="flex flex-wrap gap-x-2 py-1"><span>{row.label ?? t(row.reason==='member_hidden'?'pools.hidden':'pools.member')}</span>
        {row.reason!==null && row.reason!=='member_hidden' && <span className="text-text-muted">{choiceReason(t,row.reason)}</span>}
        {row.resource!==null && row.cost!==null && <>
          <span className="text-text-muted">{t('pools.choiceCost',{amount:row.cost})}</span>
          {row.cost_basis && <>
            <span className="text-text-muted">{t(rateSourceLabel[row.cost_basis.source])}</span>
            {row.cost_basis.uncached_input && <span className="text-text-muted">{t('usageRequest.uncached')}</span>}
          </>}
        </>}
        {row.resource!==null && row.cost===null && row.reason===null && comparesCosts && <span className="text-text-muted">{t('pools.estimateUnknown')}</span>}
      </li>)}
    </ul>
    <div className="flex justify-between gap-2"><Button size="sm" disabled={query.isFetching || after===null} onClick={()=>setAfter(null)}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={query.isFetching || next===null} onClick={()=>{if(next!==null)setAfter(next);}}>{t('documentFolders.next')}</Button></div>
  </div>;
}
