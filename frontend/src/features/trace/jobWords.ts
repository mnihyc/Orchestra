/** Retained data shows the work being done, without using a job id or protocol phase as its name. */
import type {TFunction} from 'i18next';
import { i18n } from '@/i18n';

export function retainedJobWords(t:TFunction,job:{state:string;phase?:string|null}):string{
  return job.state==='running' && job.phase!==null && job.phase!==undefined && i18n.exists(`change.phase.${job.phase}.doing`)
    ? t(`change.phase.${job.phase}.doing`,{defaultValue:t('source.job.running')})
    : i18n.exists(`source.job.${job.state}`) ? t(`source.job.${job.state}`,{defaultValue:t('trace.notRecorded')}) : t('trace.notRecorded');
}
