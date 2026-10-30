/** Reviews what a same-unit restart ends and preserves before choosing its turn grace. */
import { useMutation } from '@tanstack/react-query';
import { RotateCcw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { restartEnvironment } from '@/api/environments';
import type { Environment } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { AdminActNotice, adminSpec } from '@/ui/AdminAct';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { toast } from '@/ui/Toast';

export function RestartEnvironmentDialog({environment,onClose}:{environment:Pick<Environment,'id'|'name'> & Partial<Pick<Environment,'access'>>;onClose:()=>void}) {
  const {t}=useTranslation();
  const [interrupt,setInterrupt]=useState(false);
  const restart=useMutation({mutationFn:()=>restartEnvironment(environment.id,interrupt?'interrupt':'wait'),onSuccess:()=>{
    toast({tone:'success',title:t('environmentRestart.requested')});onClose();
  }});
  return <Dialog open title={t('environmentRestart.title',{name:environment.name})} dismissible={!restart.isPending}
    onOpenChange={(open)=>!open && !restart.isPending && onClose()} footer={<>
      <Button disabled={restart.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button icon={<RotateCcw aria-hidden />} pending={restart.isPending} onClick={()=>restart.mutate()}>{t('environmentRestart.confirm')}</Button>
    </>}>
    <AdminActNotice spec={adminSpec(environment, 'environment.manage')} />
    <div className="flex flex-col gap-4 py-2">
      <p>{t('environmentRestart.stops')}</p>
      <p>{t('environmentRestart.keeps')}</p>
      <p className="text-sm text-text-muted">{t('environmentRestart.queued')}</p>
      <Checkbox label={t('environmentRestart.interrupt')} checked={interrupt} disabled={restart.isPending} onCheckedChange={setInterrupt} />
      <p className="text-sm text-text-muted">{t(interrupt?'environmentRestart.interruptHelp':'environmentRestart.waitHelp')}</p>
      {restart.error!==null && <ErrorAlert error={restart.error} />}
    </div>
  </Dialog>;
}

/** The same review is available beside Stop in a thread's Environment panel. */
export function RestartEnvironmentButton({environment,disabled=false}:{environment:Environment;disabled?:boolean}) {
  const {t}=useTranslation();const [open,setOpen]=useState(false);
  return <>
    <Button size="sm" icon={<RotateCcw aria-hidden />} disabled={disabled} onClick={()=>setOpen(true)}>{t('environmentRestart.action')}</Button>
    {open && <RestartEnvironmentDialog environment={environment} onClose={()=>setOpen(false)} />}
  </>;
}
