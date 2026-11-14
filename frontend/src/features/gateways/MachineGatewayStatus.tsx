/** Hosts and personal-machine cards share the same connector status read. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { Pencil } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { gatewayCatalogueQuery, machineGatewayQuery, updateMachineGateway } from '@/api/gateways';
import type { MachineGatewaySetBody } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { MachineGatewayCard } from './MachineGateway';
import { GatewayChoice } from './GatewayChoice';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { ErrorAlert } from '@/ui/ErrorState';
import { toast } from '@/ui/Toast';

export function MachineGatewayStatus({ machineId, online, manage=false }: { machineId: string; online: boolean; manage?:boolean }) {
  const { t } = useTranslation();
  const status = useQuery(machineGatewayQuery(machineId, online));
  const catalogue = useQuery(gatewayCatalogueQuery);
  const [editing,setEditing]=useState(false);
  const [choice,setChoice]=useState('');
  const save=useMutation({mutationFn:(body:MachineGatewaySetBody)=>updateMachineGateway(machineId,body),onSuccess:()=>{
    toast({tone:'success',title:t('regionalGateways.saved')});setEditing(false);
  }});
  if (status.isPending || catalogue.isPending) return <SkeletonRows reads={[status, catalogue]} rows={2} />;
  if (status.isError || catalogue.isError) return <ErrorState compact title={t('regionalGateways.statusFailed')} error={status.error ?? catalogue.error}
    onRetry={() => { void status.refetch(); void catalogue.refetch(); }} />;
  return <MachineGatewayCard status={status.data} gateways={catalogue.data.gateways} refreshing={status.isFetching || catalogue.isFetching}
    onRefresh={() => { void status.refetch(); void catalogue.refetch(); }}
    actions={manage&&<IconButton label={t('regionalGateways.change')} icon={<Pencil aria-hidden/>} size="sm" pending={save.isPending} blocked={!online} reason={online?undefined:t('regionalGateways.offline')} onClick={()=>{
      setChoice(status.data.selection==='automatic'?'automatic':status.data.selected_gateway_id??'');save.reset();setEditing(!editing);
    }}/>}>
    {manage&&editing&&<form className="flex min-w-0 flex-col gap-2 border-t border-border pt-2" onSubmit={(event)=>{
      event.preventDefault();if(choice===''||!online||save.isPending)return;
      save.mutate({selection:choice==='automatic'?'automatic':'selected',gateway_id:choice==='automatic'?null:choice});
    }}>
      <GatewayChoice gateways={catalogue.data.gateways} value={choice} onChange={setChoice} keepCurrent={status.data.selection===null} disabled={save.isPending||!online}/>
      {save.error!==null&&<ErrorAlert error={save.error} message={t('regionalGateways.changeFailed')}/>}
      <div className="flex justify-end gap-2"><Button size="sm" disabled={save.isPending} onClick={()=>setEditing(false)}>{t('common.cancel')}</Button>
        <Button type="submit" size="sm" variant="primary" pending={save.isPending} disabled={choice===''||!online||choice===(status.data.selection==='automatic'?'automatic':status.data.selected_gateway_id)}>{t(save.isPending?'regionalGateways.saving':'common.save')}</Button></div>
    </form>}
  </MachineGatewayCard>;
}
