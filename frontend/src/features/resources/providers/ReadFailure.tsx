/** A broken facts projection never removes the manager's way to delete a provider. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { deleteProvider, providerKey } from '@/api/resources';
import { Page, PageHeader } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorState } from '@/ui/ErrorState';

export function ProviderReadFailure({id,error,retrying,onRetry}:{id:string;error:unknown;retrying:boolean;onRetry:()=>void}) {
  const {t}=useTranslation();const navigate=useNavigate();const cache=useQueryClient();
  const [removing,setRemoving]=useState(false);
  // Delete checks current authority and dependencies. An unreadable facts payload is never
  // turned into a synthetic provider or a guessed permission grant here.
  const remove=useMutation({mutationFn:()=>deleteProvider(id),onSuccess:()=>{
    setRemoving(false);cache.removeQueries({queryKey:providerKey(id)});void navigate({to:'/resources/providers'});
  }});
  return <Page><PageHeader title={t('screens.providers.title')} actions={<Button variant="danger-ghost" onClick={()=>setRemoving(true)}>{t('resources.act.delete')}</Button>}/>
    <ErrorState error={error} retrying={retrying} onRetry={onRetry}/>
    <ConfirmDialog open={removing} onOpenChange={(open)=>!remove.isPending && setRemoving(open)} danger pending={remove.isPending}
      error={remove.error ?? undefined} title={t('providerRecovery.deleteTitle')} confirmLabel={t('providerRecovery.delete')}
      onConfirm={()=>remove.mutate()}>{t('providers.danger.deleteBody')}</ConfirmDialog>
  </Page>;
}
