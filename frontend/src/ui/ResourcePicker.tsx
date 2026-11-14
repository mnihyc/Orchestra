/** One complete resource picker: the existing Combobox, with protected Home groups and native choices. */
import { type KeyboardEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useListChoice } from '@/lib/listState';
import { useFormatContext } from '@/lib/appearance';
import { isComposing } from '@/lib/shortcuts';
import { ResourceGroupHeader } from '@/features/resources/GroupHeader';
import { resourceGroupKey, useResourcePickerGroups } from '@/features/resources/PickerContext';
import { groupResourceRows, resourceGroupMeta, resourceHomeLabel } from '@/features/resources/groups';
import { MachineOrigin } from '@/features/resources/machines/MachineOrigin';
import { Combobox, type ComboboxGroup, type ComboboxOption, type ComboboxProps } from '@/ui/Combobox';
import type { SelectOption, SelectProps } from '@/ui/Select';
import type { ChoiceResource } from '@/ui/DetailCard';

export interface ResourcePickerProps<Value extends string> extends SelectProps<Value> {
  /** Stable purpose within its screen, independent of the translated field label. */
  pickerKey?: string;
  currentHomes?: readonly string[];
  loading?: boolean;
  footer?: ComboboxProps<Value>['footer'];
  empty?: ComboboxProps<Value>['empty'];
}

export function ResourcePicker<Value extends string>({value,onValueChange,options,pickerKey,currentHomes,valueLabel,...props}:ResourcePickerProps<Value>) {
  const {t}=useTranslation();
  const [query,setQuery]=useState('');
  const text=(option:SelectOption<Value>) => option.fullLabel ?? (typeof option.name==='string'?option.name:typeof option.label==='string'?option.label:option.preview?.label) ?? t('resourceGroups.selectionUnavailable');
  const chosen=options.find((option)=>option.value===value);
  const groups:(Omit<ComboboxGroup<Value>,'options'> & {options:ResourceLookupOption<Value>[]})[]=[];
  for(const option of options){
    const label=option.group??'';
    let group=groups.at(-1);
    if(group?.label!==label){
      group={id:`resources-${groups.length}`,label,options:[]};
      groups.push(group);
    }
    group.options.push({
      value:option.value,label:option.name??option.label,text:text(option),fullLabel:option.fullLabel,
      description:option.description,accessory:option.accessory,disabled:option.disabled,help:option.help,preview:option.preview,
      resource:option.resource,
    });
  }
  const searchKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter' || !(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    // Committing composed text is not a choice; a dismissed or empty locator never submits its form.
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  return <div className="contents" onKeyDownCapture={searchKey}><ResourcePickerLookup {...props} hint={chosen?.preview?.hint === undefined ? props.hint : <>{props.hint}{chosen.preview.hint}</>} groups={groups} query={query} onQueryChange={setQuery}
    displayValue={typeof valueLabel==='string'?valueLabel:chosen===undefined?value===''?undefined:t('resourceGroups.selectionUnavailable'):text(chosen)}
    initialActiveValue={value} pickerKey={pickerKey} currentHomes={currentHomes}
    onSelect={(option)=>{onValueChange(option.value);setQuery('');}} /></div>;
}

export interface ResourceLookupOption<Value extends string> extends ComboboxOption<Value> {resource?:ChoiceResource}
export interface ResourcePickerLookupProps<Value extends string> extends ComboboxProps<Value> {
  groups:readonly (Omit<ComboboxGroup<Value>,'options'> & {options:readonly ResourceLookupOption<Value>[]})[];
  pickerKey?:string;
  currentHomes?:readonly string[];
}

export function ResourcePickerLookup<Value extends string>({groups,query,onQueryChange,pickerKey,currentHomes,...props}:ResourcePickerLookupProps<Value>) {
  const {t}=useTranslation();
  const {locale}=useFormatContext();
  const context=useResourcePickerGroups();
  const options=groups.flatMap((group)=>group.options.map((option)=>({...option,nativeCategory:group.label})));
  const source=(option:ResourceLookupOption<Value>)=>option.resource??option.preview?.resource;
  const metadata=(option:ResourceLookupOption<Value>)=>{
    const resource=source(option);
    if(resource===undefined)return resourceGroupMeta(undefined);
    const admitted=context.entries.get(resourceGroupKey(resource.kind,resource.id));
    if(admitted!==undefined)return admitted;
    if(option.resource?.data!==undefined)return resourceGroupMeta(option.resource.data);
    return context.strict?resourceGroupMeta(undefined):resourceGroupMeta(resource.data);
  };
  const kinds=[...new Set(options.flatMap((option)=>source(option)?.kind??[]))].sort().join('-');
  const purpose=pickerKey??kinds;
  const [collapsed,setCollapsed]=useListChoice<readonly string[]>(`resource-picker.${context.screen}.${purpose}`,'collapsedHomes',[],'device');
  const [searchFolds,setSearchFolds]=useState<readonly string[]>([]);
  const wanted=query.normalize('NFKC').toLocaleLowerCase(locale).trim();
  const matches=(option:ResourceLookupOption<Value>)=>{
    const meta=metadata(option);
    return wanted==='' || [option.text,meta.home===undefined?'':resourceHomeLabel(meta,t),meta.origin_machine?.name??'']
      .some((word)=>word.normalize('NFKC').toLocaleLowerCase(locale).includes(wanted));
  };
  const chosen=options.find((option)=>option.value===props.initialActiveValue);
  const selectedHome=chosen===undefined?null:metadata(chosen).home;
  const preferred=currentHomes??[...context.currentHomes,...((selectedHome===null||selectedHome===undefined)?[]:[selectedHome.id])];
  const resources=options.filter((option)=>source(option)!==undefined&&matches(option));
  const byHome=groupResourceRows(resources,'home',metadata,t,locale,preferred);
  const special=groups.flatMap((group)=>{
    const options=group.options.filter((option)=>source(option)===undefined&&matches(option));
    return options.length===0?[]:[{...group,options}];
  });
  const folded=(id:string)=>(wanted===''?collapsed:searchFolds).includes(id);
  const toggle=(id:string)=>{
    const update=(before:readonly string[])=>before.includes(id)?before.filter((value)=>value!==id):[...before,id];
    if(wanted==='')setCollapsed(update);else setSearchFolds(update);
  };
  const offered:ComboboxGroup<Value>[]=[...special,...byHome.flatMap((group)=>{
    // Home remains the outer order and fold; native permission categories keep their own option groups.
    const sections:{category:string;rows:typeof group.rows}[]=[];
    for(const option of group.rows){
      const last=sections.at(-1);
      if(last?.category===option.nativeCategory)last.rows.push(option);
      else sections.push({category:option.nativeCategory,rows:[option]});
    }
    return sections.map((section,index)=>({id:`${group.id}-${index}`,label:section.category||group.label,collapsed:folded(group.id),
    heading:<>{index===0&&<ResourceGroupHeader group={group} kinds={[...new Set(group.rows.flatMap((option)=>source(option)?.kind??[]))]} collapsed={folded(group.id)} onToggle={()=>toggle(group.id)}/>}{!folded(group.id)&&section.category!==''&&<div aria-hidden className="px-2 py-1 text-xs font-medium text-text-subtle">{section.category}</div>}</>,
    options:section.rows.map((option)=>{
      const meta=metadata(option);
      const origin=meta.origin_machine;
      const sample=meta.availability?.length===1?meta.availability[0]:undefined;
      const reason=sample?.available===false&&option.disabled!==true&&typeof option.disabled!=='string'?sample.reason:null;
      const aliases={
        'its imported definition belongs to another machine':'otherMachine',
        'this server belongs to another environment':'otherEnvironment',
        'this server is switched off':'disabled',
        'No machine':'noMachine',
        'the machine permits no imported MCP server':'importPolicy',
        'the machine permits no MCP server added from here':'addPolicy',
        'another enabled server has the same name':'nameTaken',
        'whoever enabled it may no longer use it':'sourceUse',
        'a required capability is not granted to this environment':'capability',
      } as const;
      const alias=reason!==null&&Object.hasOwn(aliases,reason)?aliases[reason as keyof typeof aliases]:undefined;
      const unavailable=reason===null?null:alias===undefined?reason:t(`resourceGroups.reasons.${alias}`);
      return {...option,description:<>{option.description}{unavailable!==null&&<span className="block text-sm text-text-muted">{unavailable}</span>}{(origin!==null&&origin!==undefined)&&<span data-resource-origin className="flex min-w-0 items-center gap-1 text-sm text-text-muted"><MachineOrigin machine={origin}/></span>}</>};
    }),
    }));
  })];
  return <Combobox {...props} query={query} onQueryChange={(next)=>{setSearchFolds([]);onQueryChange(next);}} groups={offered}
    empty={props.empty??t('resourceGroups.noMatches')}/>;
}
