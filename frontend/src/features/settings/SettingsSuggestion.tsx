/** A single offered complete setup is a one-press fill, never a write caused by a read. */
import { useContext } from 'react';
import { ProviderSuggestionCompletion } from '@/features/settings/ProviderChoice';
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { api,client,ok } from '@/api/client';
import { defaultProviderReader } from '@/api/defaultsOpening';
import { readProviders,readProviderModels } from '@/api/resources';
import type { BackendPage } from '@/api/capabilityBackends';
import type { Provider,ProviderModel,CapabilityBackendCandidate } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import protocols from '../../../../shared/protocol-options.json';

export type SuggestionRequest = {kind:'model';purpose:'generation'|'embedding';provider?:string}
  | {kind:'backend';place:string;capability:string;provider?:string};
export type SettingsSuggestedChoice = {kind:'model';provider:Provider;model:ProviderModel;name:string}
  | {kind:'backend';provider:Provider;candidate:CapabilityBackendCandidate;name:string};
const definitions:Readonly<Record<string,{kind:string}>>=protocols.protocols;

async function discover(request:SuggestionRequest,reader:string,signal:AbortSignal):Promise<SettingsSuggestedChoice|null> {
  const current=()=>{signal.throwIfAborted();if(defaultProviderReader()!==reader)throw new DOMException('The reader changed.','AbortError');};
  current();
  const providers=await readProviders('usable',null,null,signal);
  let found:SettingsSuggestedChoice|null=null;
  for(const provider of providers) {
    current();
    if(provider.status==='disabled' || !holds(provider.access,'provider.view') || !holds(provider.access,'provider.use')
      || request.provider!==undefined && provider.id!==request.provider)continue;
    if(request.kind==='model') {
      if(provider.kind!=='api' || !provider.protocols.some((protocol)=>definitions[protocol]?.kind===request.purpose))continue;
      const models=await readProviderModels(provider.id,signal);current();
      for(const model of models.filter((model)=>model.enabled)) {
        if(found!==null)return null;
        found={kind:'model',provider,model,name:`${provider.name} · ${model.display_name || model.model_name}`};
      }
    } else {
      let cursor:string|null=null;
      const seen=new Set<string>();
      do {
        current();
        const page:BackendPage=await ok(api.GET('/providers/{id}/capability-backends',{params:{path:{id:provider.id},query:{place:request.place,capability:request.capability,...(cursor===null?{}:{cursor})}},signal}));
        current();
        for(const candidate of page.items.filter((candidate)=>candidate.eligible)) {
          if(found!==null)return null;
          found={kind:'backend',provider,candidate,name:`${provider.name} · ${candidate.offering_name}`};
        }
        cursor=page.next_cursor;
        if(cursor!==null && seen.has(cursor))throw new Error('The backend choices repeated a cursor.');
        if(cursor!==null)seen.add(cursor);
      } while(cursor!==null);
    }
  }
  current();return found;
}

export function SettingsSuggestion({request,onUse}:{request:SuggestionRequest;onUse:(value:SettingsSuggestedChoice)=>void}) {
  const {t}=useTranslation();
  const complete=useContext(ProviderSuggestionCompletion);
  const reader=useStore(client.session,(state)=>`${state.userId??''}:${state.readerVersion??0}`);
  const result=useQuery({queryKey:['setting-suggestion',reader,request],queryFn:({signal})=>discover(request,reader,signal),retry:false});
  if(result.isError)return <ErrorState compact error={result.error} onRetry={()=>void result.refetch()}/>;
  if(!result.isSuccess || result.isFetching || result.data===null)return null;
  const choice=result.data;
  return <div className="flex min-w-0 flex-wrap items-center gap-2">
    <Button size="sm" variant="ghost" className="min-w-0 max-w-full" title={choice.name} onClick={()=>{if(defaultProviderReader()===reader){onUse(choice);complete?.(choice.provider);}}}><span className="truncate">{t('settingsSuggestions.use',{name:choice.name})}</span></Button>
    <span className="text-sm text-text-muted">{t('settingsSuggestions.source')}</span>
  </div>;
}
