/** One section name in settings navigation, headings and screen identities. */
import type {TFunction} from 'i18next';
import {LABEL_SEPARATOR} from '@/lib/placeLabel';

export function sectionLabel(section:string,t:TFunction):string {
  if(section==='policies')return t('day100.names.defaults');
  if(section==='roles')return t('day100.names.roles');
  if(section==='channels'||section==='destinations')return t('day100.names.outside');
  return t(`settings.org.sectionNames.${section}`,{defaultValue:t('settings.org.sectionNames.general')});
}
export function settingsScreenTitle(section:string,place:string|null|undefined):string {return place===null||place===undefined ? section : section+LABEL_SEPARATOR+place;}
