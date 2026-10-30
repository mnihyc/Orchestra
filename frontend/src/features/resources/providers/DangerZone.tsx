/** Disable and delete: what each ends and what it leaves, with the threads that depend on the provider and Switch threads… beside a refusal.
 *
 * Disabling stops running work and keeps everything it holds, so it can be switched on again.
 * Deleting ends it while bound threads keep their history and waiting inputs. Switching threads
 * is a separate act. Both ask
 * once, naming the provider and saying what happens. A token somebody pasted stays valid at its
 * vendor until it is revoked there, and the dialog says so.
 */
import { useMutation,useQuery } from '@tanstack/react-query';
import { Ban, CirclePlay } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { resourceImpactQuery } from '@/api/resource-impact';
import { ImpactCounts } from '@/features/storage/Impact';
import { useResourceEnable } from '@/features/storage/Enable';
import { openChange } from '@/features/change/target';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Input } from '@/ui/Input';
import { deleteProvider, disableProvider, enableProvider } from '@/api/resources';
import type { Provider } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { adminSpec } from '@/ui/AdminAct';
import { toast } from '@/ui/Toast';

export function DisableProviderDialog({ provider, open, onClose }: { provider: Provider; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const impact=useQuery({...resourceImpactQuery({object:{kind:'provider',id:provider.id},model_id:null}),enabled:open});
  const [note,setNote]=useState('');
  const disable = useMutation({
    mutationFn: () => note.trim()===''?disableProvider(provider.id):disableProvider(provider.id,note.trim()),
    onSuccess: () => {
      toast({ tone: 'success', title: t('providers.danger.disabled', { name: provider.name }) });
      onClose();
    },
  });
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('providers.danger.disableTitle', { name: provider.name })}
      admin={adminSpec(provider, 'provider.manage')}
      confirmLabel={t('providers.danger.disableConfirm', { name: provider.name })}
      danger
      pending={disable.isPending}
      error={disable.error ?? undefined}
      disabled={impact.isPending||impact.isError}
      onConfirm={() => disable.mutate()}
    >
      <div className="flex flex-col gap-3">{impact.isPending?<SkeletonRows rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<ImpactCounts impact={impact.data} kind="disable" />}<Input label={t('resourceImpact.note')} optional value={note} onChange={(event)=>setNote(event.target.value)} disabled={disable.isPending} /><ArticleGuideLink article="pause-and-switch" heading="disable" title={t('resourceImpact.guide')} /></div>
    </ConfirmDialog>
  );
}

export function DeleteProviderDialog({ provider, open, onClose, afterDelete }: { provider: Provider; open: boolean; onClose: () => void; afterDelete?: () => void }) {
  const { t } = useTranslation();
  const impact=useQuery({...resourceImpactQuery({object:{kind:'provider',id:provider.id},model_id:null}),enabled:open});
  const [typed,setTyped]=useState('');
  const remove = useMutation({
    mutationFn: () => deleteProvider(provider.id),
    onSuccess: () => {
      toast({ tone: 'success', title: t('providers.danger.deleted', { name: provider.name }) });
      onClose();
      afterDelete?.();
    },
  });
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('providers.danger.deleteTitle', { name: provider.name })}
      admin={adminSpec(provider, 'provider.manage')}
      confirmLabel={t('providers.danger.deleteConfirm', { name: provider.name })}
      danger
      pending={remove.isPending}
      error={remove.error ?? undefined}
      disabled={impact.isPending||impact.isError||typed!==(impact.data.name)}
      onConfirm={() => remove.mutate()}
    >
      <span className="flex flex-col gap-2">
        {impact.isPending?<SkeletonRows rows={2} />:impact.isError?<ErrorState compact error={impact.error} onRetry={()=>void impact.refetch()} />:<ImpactCounts impact={impact.data} kind="delete" />}
        <span>{t('resourceImpact.deleteKeeps')}</span>
        <span>{provider.kind === 'subscription' ? t('providers.danger.deleteToken') : t('providers.danger.deleteKey')}</span>
        <Button variant="ghost" size="sm" onClick={()=>{onClose();openChange({kind:'provider',id:provider.id,name:provider.name},{kind:'switch'});}}>{t('resourceImpact.switchFirst')}</Button>
        <ArticleGuideLink article="pause-and-switch" heading="delete" title={t('resourceImpact.guide')} />
        <Input label={t('resourceImpact.typeName',{name:impact.data?.name ?? provider.name})} value={typed} onChange={(event)=>setTyped(event.target.value)} autoComplete="off" />
      </span>
    </ConfirmDialog>
  );
}

/** The last tab of a provider's page: what can be switched off or ended, and what each does. */
export function ProviderDangerZone({ provider, onDisable }: { provider: Provider; onDisable: () => void }) {
  const { t } = useTranslation();
  const enable = useResourceEnable();
  const disabled = provider.status === 'disabled';
  return (
    <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-sm font-medium">{t('providerRecovery.availability')}</span>
          {disabled ? (
            <Button icon={<CirclePlay aria-hidden />} onClick={() => enable.ask({target:{object:{kind:'provider',id:provider.id},model_id:null},name:provider.name,admin:adminSpec(provider,'provider.manage'),enable:(runWaiting)=>enableProvider(provider.id,runWaiting)})}>
              {t('providers.danger.enable')}
            </Button>
          ) : (
            <Button icon={<Ban aria-hidden />} onClick={onDisable}>
              {t('providers.danger.disable')}
            </Button>
          )}
      {enable.dialogs}
    </div>
  );
}
