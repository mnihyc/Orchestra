/** A model's pause and deletion show their exact current effects without moving its threads. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { deleteModel, updateModel } from '@/api/resources';
import { resourceImpactQuery } from '@/api/resource-impact';
import { openChange } from '@/features/change/target';
import { StopTip } from '@/features/storage/Impact';
import { UpdateNotify } from '@/features/change/UpdateNotify';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import type { Provider, ProviderModel } from '@/generated/api-types';
import { adminSpec } from '@/ui/AdminAct';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

export function ModelLifecycle({ provider, model, kind, onClose }: {provider:Provider;model:ProviderModel;kind:'disable'|'delete';onClose:()=>void}) {
  const {t}=useTranslation();
  const impact=useQuery(resourceImpactQuery({object:{kind:'provider',id:provider.id},model_id:model.id}, {operation_notice:true}));
  const [note,setNote]=useState(''),[typed,setTyped]=useState('');
  const [notify,setNotify]=useState<boolean>();
  const name=(impact.data?.name===''?undefined:impact.data?.name)??(model.display_name===''?undefined:model.display_name)??model.model_name;
  const change=useMutation({mutationFn:async()=>{
    const chosen = notify ?? impact.data?.notification?.default_on ?? false;
    if(kind==='delete')await deleteModel(provider.id,model.id,chosen);
    else await updateModel(provider.id,model.id,{enabled:false},note.trim()===''?undefined:note.trim(),undefined,chosen);
  },onSuccess:()=>{
    toast({tone:'success',title:t(kind==='delete'?'providers.models.removed':'providers.danger.disabled',{name})});onClose();
  }});
  return <ConfirmDialog open onOpenChange={(open)=>!open&&onClose()} admin={adminSpec(provider,'provider.manage')}
    title={t(kind==='delete'?'providers.danger.deleteTitle':'providers.danger.disableTitle',{name})}
    confirmLabel={t(kind==='delete'?'providers.danger.deleteConfirm':'providers.danger.disableConfirm',{name})}
    footerStart={<UpdateNotify audience={impact.data?.notification?.audience} checked={notify ?? impact.data?.notification?.default_on ?? false} onChange={setNotify} disabled={change.isPending} />}
    danger pending={change.isPending} error={change.error??undefined} disabled={kind==='delete'&&typed!==name} onConfirm={()=>change.mutate()}>
    <div className="flex min-w-0 flex-col gap-3">
      {(impact.isPending || impact.isError) && <span className="text-sm text-warning-text">{t('resourceImpact.stopTip')}</span>}{impact.isPending?<SkeletonRows read={impact} rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<StopTip impact={impact.data} kind={kind} />}
      {kind==='disable'?<Input optional label={t('resourceImpact.note')} value={note} onChange={(event)=>setNote(event.target.value)} disabled={change.isPending} />:<>
        <span className="text-sm text-text-muted">{t('resourceImpact.deleteKeeps')}</span>
        <Button size="sm" variant="ghost" onClick={()=>{onClose();openChange({kind:'model',providerId:provider.id,id:model.id,name},{kind:'switch'});}}>{t('resourceImpact.switchFirst')}</Button>
        <Input label={t('resourceImpact.typeName',{name})} value={typed} onChange={(event)=>setTyped(event.target.value)} autoComplete="off" disabled={change.isPending} />
      </>}
      <ArticleGuideLink article="pause-and-switch" heading={kind} title={t('resourceImpact.guide')} />
    </div>
  </ConfirmDialog>;
}
