/** A destination uses its native name page, without loading every organization's tree. */
import {useInfiniteQuery} from '@tanstack/react-query';
import {X} from 'lucide-react';
import {useEffect,useMemo,useState} from 'react';
import {useTranslation} from 'react-i18next';
import {useStore} from 'zustand';
import {client} from '@/api/client';
import {pickerNamesQuery,type NameKind} from '@/api/search';
import type {AdminAccess,NameHit,NodeKind,NodeRef} from '@/generated/api-types';
import {useDebounced} from '@/lib/debounce';
import {usePickerCursorRestart} from '@/lib/pickerPages';
import {AsAdminMark,adminAccessOf} from '@/ui/AsAdminMark';
import {ResourcePickerLookup} from '@/ui/ResourcePicker';
import {IconButton} from '@/ui/IconButton';
import type {ChoiceHelp} from '@/ui/Choice';
import {NodeIcon} from '@/ui/NodeIcon';
import {Button} from '@/ui/Button';
import {ErrorState} from '@/ui/ErrorState';

export interface PickedPlace {
  adminAccess?:AdminAccess;
  rootOrgId?:string;
  rootName?:string;
  id:string;
  kind:NodeKind;
  color:number|null;
  custom_color?:string|null;
  folder:boolean;
  path:string;
}

/** Only the native reply supplies names; unavailable ancestors stay unnamed. */
export function placesOfNames(rows:readonly NameHit[],separator:string,hidden:string,includeThreads=false,rootId?:string):PickedPlace[] {
  return rows.flatMap((hit)=>{
    const kind=hit.kind;
    if(kind!=='org'&&kind!=='project'&&kind!=='topic'&&kind!=='thread')return [];
    if(kind==='thread'&&!includeThreads)return [];
    const self:NodeRef={id:hit.id,kind,name:hit.name,redacted:false};
    const path=hit.path.at(-1)?.id===hit.id?[...hit.path.slice(0,-1),self]:[...hit.path,self];
    const root=rootId??(path[0]?.kind==='org'?path[0].id:undefined);
    const namedRoot=path.find((node)=>node.id===root&&!node.redacted&&node.name!==null);
    const admin=adminAccessOf(hit);
    return [{id:hit.id,kind,color:hit.color??null,custom_color:hit.custom_color,
      folder:kind==='project'&&path.at(-2)?.kind==='project',
      path:path.map((node)=>node.redacted||node.name===null?hidden:node.name).join(separator),
      ...(root===undefined?{}:{rootOrgId:root}),...(namedRoot?.name===null||namedRoot?.name===undefined?{}:{rootName:namedRoot.name}),
      ...(admin===null?{}:{adminAccess:admin}),
    }];
  });
}

export interface PlacePickerProps {
  label: string;
  hint?: string;
  value: PickedPlace | null;
  onChange: (place: PickedPlace | null) => void;
  disabled?: boolean;
  /** Restricts environment homes to their existing root org. */
  rootOrgId?: string;
  /** Offers only places of this kind: the topic a thread is made in. */
  only?: NodeKind;
  /** Settings can be applied to a thread as well as the places above it. */
  includeThreads?: boolean;
  /** Native destination rules keep an unavailable place visible, with its own reason. */
  refusal?: (place: PickedPlace) => string | false;
  onCandidatesChange?: (places: readonly PickedPlace[]) => void;
  optionHelp?: (place: PickedPlace) => ChoiceHelp | undefined;
  optionDescription?: (place: PickedPlace) => string | undefined;
}

export function PlacePicker({label,hint,value,onChange,disabled=false,rootOrgId,only,includeThreads=false,refusal,onCandidatesChange,optionHelp,optionDescription}:PlacePickerProps) {
  const {t}=useTranslation();
  const reader=useStore(client.session,(state)=>`${state.userId}:${state.readerVersion}`);
  const [query,setQuery]=useState('');const [open,setOpen]=useState(false);
  const text=query.trim(),rested=useDebounced(text),waiting=text!==rested;
  const kinds:NameKind[]=only===undefined?['org','project','topic',...(includeThreads?['thread' as const]:[])]:[only];
  const scope=JSON.stringify([reader,rootOrgId,kinds,rested]);
  const [page,setPage]=useState({scope,index:0});
  if(page.scope!==scope)setPage({scope,index:0});
  const request=pickerNamesQuery({q:rested,kinds,...(rootOrgId===undefined?{}:{orgId:rootOrgId})});
  const source=useInfiniteQuery({...request,enabled:open&&value===null&&!waiting});
  usePickerCursorRestart(source,request.queryKey);
  const pages=source.data?.pages??[];
  const index=page.scope===scope?Math.min(page.index,Math.max(0,pages.length-1)):0;
  if(source.data!==undefined&&page.scope===scope&&page.index!==index)setPage({scope,index});
  const rows=pages[index]?.items;
  const places=useMemo(()=>waiting||source.isError?[]:placesOfNames(rows??[],t('settings.place.separator'),t('access.place.hidden'),includeThreads||only==='thread',rootOrgId),[rows,t,waiting,source.isError,includeThreads,only,rootOrgId]);
  useEffect(()=>{onCandidatesChange?.(places);},[places,onCandidatesChange]);
  const more=index+1<pages.length||source.hasNextPage;
  const next=async()=>{
    if(index+1>=pages.length){const result=await source.fetchNextPage();if(result.isError||result.data===undefined||result.data.pages.length<=index+1)return;}
    setPage((before)=>before.scope===scope&&before.index===index?{scope,index:index+1}:before);
  };
  const retry=()=>{setPage({scope,index:0});void source.refetch();};
  if(value!==null)return <div className="flex flex-col gap-1">
    <span className="text-sm font-medium text-text">{label}</span>
    <div className="flex items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
      <NodeIcon kind={value.folder?'folder':value.kind} color={value.color} own={value.custom_color} className="size-4"/>
      <span className="min-w-0 flex-1 text-base wrap-anywhere text-text">{value.path}</span><AsAdminMark of={value.adminAccess?.place}/>
      <IconButton size="sm" label={t('settings.place.clear',{path:value.path})} icon={<X aria-hidden/>} disabled={disabled} onClick={()=>onChange(null)}/>
    </div>{refusal?.(value)&&<span className="text-sm text-text-muted">{refusal(value)}</span>}
  </div>;
  return <ResourcePickerLookup pickerKey="place-destination" groupHomes={false} filterLocal={false}
    label={label} hint={hint} query={query} onQueryChange={setQuery} onOpenChange={setOpen} disabled={disabled}
    loading={open&&(waiting||source.isPending)} empty={t('settings.place.none')} placeholder={t('settings.place.placeholder')}
    groups={[{id:'places',label:t('settings.place.group'),options:places.map((place)=>({value:place.id,text:place.path,
      resource:{kind:place.kind,id:place.id},
      label:<span className="flex min-w-0 items-center gap-2"><span>{place.path}</span><AsAdminMark of={place.adminAccess?.place}/></span>,
      icon:<NodeIcon kind={place.folder?'folder':place.kind} color={place.color} own={place.custom_color}/>,
      description:optionDescription?.(place)??t(`settings.place.kinds.${place.kind}`),help:optionHelp?.(place),disabled:refusal?.(place)??false,
    }))}]}
    footer={source.isError?<ErrorState compact error={source.error} onRetry={retry}/>:index>0||more?<div className="flex justify-end gap-2">
      <Button size="sm" variant="ghost" disabled={index===0||waiting} onClick={()=>setPage({scope,index:index-1})}>{t('resourceDetails.layout.previous')}</Button>
      <Button size="sm" variant="ghost" disabled={!more||waiting} pending={source.isFetchingNextPage} onClick={()=>{void next();}}>{t('resourceDetails.layout.next')}</Button>
    </div>:undefined}
    onSelect={(option)=>{const place=places.find((row)=>row.id===option.value);if(place!==undefined){setOpen(false);setQuery('');onChange(place);}}}/>
}
