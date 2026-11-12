/** Service quantities remain their declared units, with coverage kept separate from their known sum. */
import type {TFunction} from 'i18next';
import type {ServiceUsage,ServiceUnitTotals} from '@/generated/api-types';
import {formatNumber} from '@/lib/format';

export function serviceUnitName(unit:string,t:TFunction):string {
  return t(`usage.serviceUnits.unit.${unit}` as 'usage.serviceUnits.unit.call',{defaultValue:unit});
}
export function serviceQuantityWords(unit:string,value:number|null,t:TFunction,locale:string):string {
  const shown=value===null?'–':formatNumber(value,locale);
  return t(`usage.serviceUnits.quantity.${unit}` as 'usage.serviceUnits.quantity.call',{count:value??0,value:shown,defaultValue:`${shown} ${unit}`});
}
export function serviceUnitsWords(units:Record<string,ServiceUnitTotals>|undefined|null,t:TFunction,locale:string):string {
  return Object.entries(units??{}).map(([unit,total])=>[
    serviceQuantityWords(unit,total.known_requests===0?null:total.quantity,t,locale),
    ...(total.unknown_requests===0?[]:[t('usage.serviceUnits.unknown',{count:total.unknown_requests,value:formatNumber(total.unknown_requests,locale)})]),
  ].join(' · ')).join(' · ');
}
export function serviceQuoteWords(service:ServiceUsage,t:TFunction):string {
  const quote=service.price;
  return quote===undefined || quote===null?t('usage.records.noRate'):t('usage.serviceUnits.quote',{amount:quote.amount,currency:quote.currency.toLowerCase()==='credits'?t('caps.unit.credits'):quote.currency,unit:serviceUnitName(quote.unit,t)});
}
