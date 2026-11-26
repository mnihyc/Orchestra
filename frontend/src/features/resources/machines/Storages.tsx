/** Named storage capacity comes from machine facts, with no inference from its roles or placement. */
import type { MachineDataInventory } from '@/generated/api-types';
import { MachineDataDirectory } from './DataDirectory';
import { useTranslation } from 'react-i18next';
import { diskWords } from '@/features/resources/amounts';
import { useFormatContext } from '@/lib/appearance';
import { EmptyState } from '@/ui/EmptyState';
import { record } from '@/features/resources/UnitCommands';
import { Facts } from '@/features/resources/Facts';
import { Section } from '@/shell/Page';

export function MachineStorages({ facts, dataDirectory }: { facts: unknown; dataDirectory?: MachineDataInventory | null }) {
  const { t }=useTranslation();
  const { locale }=useFormatContext();
  const offered=record(facts).storages;
  const rows=Array.isArray(offered)?offered.flatMap((value)=>{
    const row=record(value);
    if(typeof row.name!=='string'||typeof row.capacity_gb!=='number'||typeof row.free_gb!=='number')return [];
    return [{name:row.default===true?t('machineStorage.default',{name:row.name}):row.name,value:t('resourceFacts.diskFree',{free:diskWords(Math.max(0,row.free_gb),t,locale),total:diskWords(row.capacity_gb,t,locale)})}];
  }):[];
  return <><Section id="machine-storages" title={t('machineStorage.title')} lead={t('machineStorage.lead')}>
    {rows.length===0?<EmptyState compact title={t('machineStorage.none')} />:<Facts label={t('machineStorage.title')} rows={rows} />}
  </Section><MachineDataDirectory inventory={dataDirectory} /></>;
}
