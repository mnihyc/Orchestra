/** Named storage capacity comes from machine facts, with no inference from its roles or placement. */
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { record } from '@/features/resources/UnitCommands';
import { Facts } from '@/features/resources/Facts';
import { Section } from '@/shell/Page';

export function MachineStorages({ facts }: { facts: unknown }) {
  const { t }=useTranslation();
  const offered=record(facts).storages;
  const rows=Array.isArray(offered)?offered.flatMap((value)=>{
    const row=record(value);
    if(typeof row.name!=='string'||typeof row.capacity_gb!=='number'||typeof row.free_gb!=='number')return [];
    return [{name:row.default===true?t('machineStorage.default',{name:row.name}):row.name,value:t('machineStorage.space',{free:row.free_gb,capacity:row.capacity_gb})}];
  }):[];
  return <Section id="machine-storages" title={t('machineStorage.title')} lead={t('machineStorage.lead')}>
    {rows.length===0?<EmptyState compact title={t('machineStorage.none')} />:<Facts label={t('machineStorage.title')} rows={rows} />}
  </Section>;
}
