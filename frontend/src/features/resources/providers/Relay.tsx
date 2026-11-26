/** Public model access is an explicit provider choice; models stay in one closed searchable picker. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BASE_PATH } from '@/api/client';
import { providerKey, providerModelsQuery, updateProvider } from '@/api/resources';
import type { Provider, ProviderModel, ProviderRelayOptions } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { adminSpec } from '@/ui/AdminAct';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';

type Offering=Pick<ProviderModel,'model_name'|'display_name'|'enabled'>;
const names=(models:readonly string[])=>JSON.stringify([...models].sort());

export function RelayModels({saved,models,pending,onSave}:{saved:string[];models:Offering[];pending:boolean;onSave:(models:string[])=>void}) {
  const {t}=useTranslation();
  const [base,setBase]=useState(saved);
  const [draft,setDraft]=useState(saved);
  const [search,setSearch]=useState('');
  const dirty=names(draft)!==names(base);
  const changed=names(saved)!==names(base);
  if(changed&&!dirty){setBase(saved);setDraft(saved);}
  const rows=[...models,...draft.filter((name)=>!models.some((model)=>model.model_name===name)).map((name)=>({model_name:name,display_name:'',enabled:false}))];
  const found=rows.filter((model)=>(model.display_name+' '+model.model_name).toLocaleLowerCase().includes(search.toLocaleLowerCase()));
  return <Popover label={t('providerRelay.chooseModels')} scroll="children" onOpenChange={(open)=>{if(!open)setSearch('');}}
    trigger={<Button size="sm" variant="secondary">{t('providerRelay.models',{count:saved.length})}</Button>}>
    <Input type="search" label={t('providerRelay.search')} value={search} onChange={(event)=>setSearch(event.target.value)}/>
    <div className="my-2 flex max-h-64 min-h-0 flex-col gap-2 overflow-y-auto" role="group" aria-label={t('providerRelay.chooseModels')}>
      {found.map((model)=><Checkbox key={model.model_name} label={model.display_name||model.model_name} presentation="picker"
        checked={draft.includes(model.model_name)} disabled={pending||(!model.enabled&&!draft.includes(model.model_name))}
        tooltip={model.enabled?undefined:t('providerRelay.modelDisabled')} onCheckedChange={(checked)=>setDraft((before)=>checked?[...before,model.model_name]:before.filter((name)=>name!==model.model_name))}/>)}
      {found.length===0&&<span className="text-sm text-text-muted">{t('providerRelay.noModels')}</span>}
    </div>
    {changed&&dirty&&<span className="text-sm text-danger">{t('providerRelay.changed')}</span>}
    <div className="flex justify-end gap-2">
      {changed&&dirty&&<Button size="sm" variant="secondary" onClick={()=>{setBase(saved);setDraft(saved);}}>{t('providerRelay.reload')}</Button>}
      <Button size="sm" disabled={!dirty||changed||pending} pending={pending} onClick={()=>onSave([...draft].sort())}>{t('common.save')}</Button>
    </div>
  </Popover>;
}

export function ProviderRelay({provider}:{provider:Provider}) {
  if(provider.kind!=='api'||provider.relay===undefined||provider.relay===null)return null;
  return <RelayControl provider={provider} saved={provider.relay}/>;
}
function RelayControl({provider,saved}:{provider:Provider;saved:ProviderRelayOptions}) {
  const {t}=useTranslation();
  const client=useQueryClient();
  const models=useQuery(providerModelsQuery(provider.id));
  const change=useMutation({mutationFn:(relay:ProviderRelayOptions)=>updateProvider(provider.id,{expected_revision:provider.revision,relay}),
    onSuccess:()=>client.invalidateQueries({queryKey:providerKey(provider.id)}),
    onError:()=>void client.invalidateQueries({queryKey:providerKey(provider.id)}),
  });
  const base=new URL(`${BASE_PATH}/providers/${encodeURIComponent(provider.id)}/relay`,window.location.origin).href;
  return <Section id="provider-relay" title={t('providerRelay.title')}>
    <div className="flex flex-wrap items-center justify-between gap-2">
      <Switch presentation="compact" label={t(change.isPending&&change.variables.enabled!==saved.enabled?(change.variables.enabled?'providerRelay.turningOn':'providerRelay.turningOff'):(saved.enabled?'providerRelay.on':'providerRelay.off'))} checked={saved.enabled}
        admin={adminSpec(provider,'provider.manage')} disabled={change.isPending} onCheckedChange={(enabled)=>change.mutate({...saved,enabled})}/>
      {models.isPending?<SkeletonRows read={models} rows={1}/>:models.isError?<ErrorState compact error={models.error} retrying={models.isRefetching} onRetry={()=>void models.refetch()}/>:
        <RelayModels saved={saved.models} models={models.data} pending={change.isPending} onSave={(opened)=>change.mutate({...saved,models:opened})}/>}
    </div>
    <div className="flex min-w-0 items-center gap-2 text-xs text-text-muted"><span>{t('providerRelay.base')}</span><code className="min-w-0 break-all">{base}</code><CopyButton value={base} label={t('providerRelay.copyBase')}/></div>
    {change.isError&&<ErrorAlert error={change.error}/>}
  </Section>;
}
