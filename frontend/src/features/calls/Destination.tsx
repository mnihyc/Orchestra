/** Destination names use their current native reader; a hidden pool never exposes its members. */
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { poolQuery } from '@/api/pools';
import type { ResourceTarget } from '@/generated/api-types';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { CallModel } from './Model';

function PoolName({id}:{id:string}) {
  const {t}=useTranslation();
  const reader=useStore(client.session,(state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const query=useQuery({...poolQuery(id,reader),retry:false});
  if(query.isError) return <span>{t('settings.compact.managed')}</span>;
  if(query.isPending) return <span>{t('common.loading')}</span>;
  return <ResourceChip subject={{kind:'pool',pool:query.data}} hint={false} />;
}
export function CallDestination({target}:{target:ResourceTarget}) {
  const {t}=useTranslation();
  if(target.type==='pool') return <PoolName id={target.id} />;
  if(target.resource.kind==='model' && target.resource.backend.route==='generation.model' && target.resource.backend.offering!==null) return <CallModel providerId={target.resource.backend.offering.provider} model={target.resource.backend.offering.offering} />;
  return <span>{t(target.resource.kind==='provider'?'calls.provider':'calls.capabilityDestination')}</span>;
}
