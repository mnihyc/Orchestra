import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { providersQuery } from '@/api/resources';
import { Input } from '@/ui/Input';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

/** The ordinary readable provider list supplies reciprocal links, without exposing hidden rows. */
export function EnvironmentProviderConnections({environmentId}:{environmentId:string}) {
  const {t}=useTranslation();
  const list=useQuery(providersQuery('all',null,environmentId));
  const [query,setQuery]=useState('');
  if(list.isPending)return <SkeletonRows rows={2} read={list}/>;
  if(list.isError)return <ErrorState compact error={list.error} onRetry={()=>void list.refetch()}/>;
  const rows=list.data;
  const match=query.trim().normalize('NFKC').toLocaleLowerCase();
  const filtered=rows.filter((provider)=>provider.name.normalize('NFKC').toLocaleLowerCase().includes(match));
  return <div className="flex min-w-0 flex-col gap-2">
    {rows.length>0 && <span className="text-sm text-text-muted">{t('providerCreation.environmentConnectionsFact')}</span>}
    {rows.length>10 && <Input label={t('providerCreation.searchConnections')} hideLabel value={query} onChange={(event)=>setQuery(event.target.value)}/>}
    <div className="max-h-64 overflow-y-auto">
      {filtered.length===0 ? <span className="text-sm text-text-muted">{t('providerCreation.environmentConnectionsEmpty')}</span>
        : <ul className="flex flex-col gap-1">{filtered.map((provider)=><li key={provider.id}><ResourceName subject={{kind:"provider",provider}} asChild hint={false}><Link to="/resources/providers/$id" params={{id:provider.id}}>{provider.name}</Link></ResourceName></li>)}</ul>}
    </div>
  </div>;
}
