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
import {recentChoiceRows,usePickerRecents} from '@/lib/pickerRecents';
import {useRef,useCallback} from 'react';
import {Tree,type TreeRow} from '@/ui/Tree';
import {Popover} from '@/ui/Popover';
import {Input,fieldBoxClasses} from '@/ui/Input';
import {Field} from '@/ui/Field';
import {Choice} from '@/ui/Choice';
import {ChevronDown} from 'lucide-react';
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

/** Build just the disclosed paths leading to this native page. Ancestors provide navigation. */
export function placeHierarchy(hits: readonly NameHit[], places: readonly PickedPlace[], separator: string, hidden: string): TreeRow<PickedPlace>[] {
  const selected = new Map(places.map(place => [place.id, place]));
  const nodes = new Map<string, PickedPlace>();
  const children = new Map<string | null, string[]>();
  for (const hit of hits) {
    const place = selected.get(hit.id);
    if (place === undefined) continue;
    const path = [...hit.path.filter(node => node.id !== hit.id), {id:hit.id,kind:place.kind,name:hit.name,redacted:false}];
    const names:string[]=[];
    let parent:string|null=null;
    for(const node of path){
      names.push(node.redacted || node.name===null ? hidden : node.name);
      if(node.redacted || node.name===null)continue;
      if(!nodes.has(node.id)){
        nodes.set(node.id,selected.get(node.id) ?? {id:node.id,kind:node.kind,color:null,folder:false,path:names.join(separator)});
        const siblings=children.get(parent);
        if(siblings===undefined)children.set(parent,[node.id]);else siblings.push(node.id);
      }
      parent=node.id;
    }
  }
  const stack=(children.get(null)??[]).toReversed().map(id=>({id,parentId:null as string|null,depth:0}));
  const rows:TreeRow<PickedPlace>[]=[];
  const visited=new Set<string>();
  while(stack.length){
    const next=stack.pop();
    if(next===undefined || visited.has(next.id))continue;
    visited.add(next.id);
    const item=nodes.get(next.id);
    if(item===undefined)continue;
    const below=children.get(next.id)??[];
    rows.push({...next,item,label:item.path,expandable:below.length>0,expanded:true});
    for(const id of below.toReversed())stack.push({id,parentId:next.id,depth:next.depth+1});
  }
  return rows;
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
  const more=index+1<pages.length||source.hasNextPage;
  const next=async()=>{
    if(index+1>=pages.length){const result=await source.fetchNextPage();if(result.isError||result.data===undefined||result.data.pages.length<=index+1)return;}
    setPage((before)=>before.scope===scope&&before.index===index?{scope,index:index+1}:before);
  };
  const retry=()=>{setPage({scope,index:0});void source.refetch();};

  const recent = usePickerRecents(open);
  const search = useRef<HTMLInputElement>(null);
  const holder = useRef<HTMLDivElement>(null);
  const [closed,setClosed] = useState<ReadonlySet<string>>(() => new Set());
  const hierarchy = useMemo(() => placeHierarchy(rows ?? [], places, t('settings.place.separator'), t('access.place.hidden')), [rows,places,t]);
  const candidateIds = useMemo(() => new Set(places.map((p) => p.id)), [places]);
  const treeRows = useMemo(() => {
    const recentPlaces = text === '' ? recentChoiceRows(places,Object.entries(recent.choices ?? {}).flatMap(([kind,ids]) => ids.slice(0,recent.count).map((id) => kind+':'+id)),p=>p.kind+':'+p.id,p=>!refusal?.(p)) : [];
    const promoted = new Set(recentPlaces.map((p)=>p.id));
    const output: TreeRow<PickedPlace>[] = [];
    const firstRecent = recentPlaces[0];
    if (firstRecent !== undefined) {
      const expanded = !closed.has('recent-places');
      output.push({ id: 'recent-places', item: firstRecent, label: t('ui.picker.recent'), depth: 0, parentId: null, expandable: true, expanded, disabled: true });
      if (expanded) for (const place of recentPlaces) {
        output.push({ id: 'recent:'+place.id, item: place, label: place.path, depth: 1, parentId: 'recent-places', expandable: false, expanded: false });
      }
    }
    const hidden = new Set<string>();
    for (const row of hierarchy) {
      if (row.parentId !== null && hidden.has(row.parentId)) { hidden.add(row.id); continue; }
      const expanded = text !== '' || !closed.has(row.id);
      if (!expanded) hidden.add(row.id);
      if (promoted.has(row.id) && !row.expandable) continue;
      const eligible = candidateIds.has(row.id) && !promoted.has(row.id);
      const reason = eligible ? refusal?.(row.item) : false;
      output.push({...row,expanded,disabled:!eligible || Boolean(reason),description:typeof reason==='string'?reason:optionDescription?.(row.item) ?? t(`settings.place.kinds.${row.item.kind}`)});
    }
    return output;
  },[places,recent.choices,recent.count,text,hierarchy,closed,candidateIds,refusal,optionDescription,t]);
  const visibleCandidates = useMemo(() => {
    const visible = new Set(treeRows.filter(row => row.id !== 'recent-places').map(row => row.item.id));
    return places.filter(place => visible.has(place.id));
  }, [treeRows,places]);
  useEffect(()=>{onCandidatesChange?.(open ? visibleCandidates : []);},[open,visibleCandidates,onCandidatesChange]);
  const choose = (row:TreeRow<PickedPlace>) => {
    if(row.disabled || disabled)return;
    recent.record({kind:row.item.kind,key:row.item.id});onChange(row.item);setQuery('');setOpen(false);
  };
  const renderRow=useCallback((row:TreeRow<PickedPlace>)=>{
    if (row.id === 'recent-places') return <span>{t('ui.picker.recent')}</span>;
    const refusalReason = candidateIds.has(row.item.id) ? refusal?.(row.item) : undefined;
    const description = typeof refusalReason === 'string' ? refusalReason : optionDescription?.(row.item);
    return <span className="flex min-w-0 flex-1 items-center gap-2" title={row.item.path}>
    <NodeIcon kind={row.item.folder?'folder':row.item.kind} color={row.item.color} own={row.item.custom_color}/>
    <Choice presentation="picker" name={row.item.path} help={candidateIds.has(row.item.id)?optionHelp?.(row.item):undefined}/>
    <AsAdminMark of={row.item.adminAccess?.place} iconOnly/>
    {candidateIds.has(row.item.id) && description && <span className="ml-auto text-xs text-text-muted">{description}</span>}
  </span>;},[candidateIds,optionHelp,refusal,optionDescription,t]);
  const loading=waiting || source.isPending || recent.loading;
  if (value !== null) {
    return (
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium text-text">{label}</span>
        <div className="flex items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
          <NodeIcon kind={value.folder ? 'folder' : value.kind} color={value.color} own={value.custom_color} className="size-4" />
          <span className="min-w-0 flex-1 text-base wrap-anywhere text-text">{value.path}</span>
          <AsAdminMark of={value.adminAccess?.place} />
          <IconButton size="sm" label={t('settings.place.clear', { path: value.path })} icon={<X aria-hidden />} disabled={disabled} onClick={() => onChange(null)} />
        </div>
        {refusal?.(value) && <span className="text-sm text-text-muted">{refusal(value)}</span>}
      </div>
    );
  }
  return <Field label={label} hint={hint}>{(control) => <Popover label={label} open={open} onOpenChange={setOpen} scroll="children"
    className="w-(--radix-popover-trigger-width) min-w-64 gap-2 p-2"
    onOpenAutoFocus={(event) => { event.preventDefault(); search.current?.focus(); }}
    trigger={<button type="button" {...control} role="combobox" aria-haspopup="tree" aria-expanded={open} disabled={disabled} className={`${fieldBoxClasses} flex items-center gap-2 text-left`}>
      <span className="min-w-0 flex-1 truncate text-text-subtle">{t('settings.place.placeholder')}</span><ChevronDown aria-hidden className="size-4 shrink-0 text-text-subtle" />
    </button>}>
    <Input ref={search} label={t('settings.place.placeholder')} hideLabel type="search" value={query} onChange={(event) => setQuery(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === 'ArrowDown') { event.preventDefault(); holder.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]')?.focus(); }
        if (event.key === 'Escape' && query !== '') { event.preventDefault(); event.stopPropagation(); setQuery(''); }
      }} />
    {loading ? <p role="status" className="text-sm text-text-muted">{t('common.loading')}</p> : treeRows.length === 0 ? <p className="text-sm text-text-muted">{t('settings.place.none')}</p> : <div ref={holder} className="min-h-0 max-h-80 overflow-y-auto overscroll-contain">
      <Tree label={label} rows={treeRows} nameOf={(place) => place.path} renderRow={renderRow} onPick={choose}
        onExpandedChange={(id, expanded) => setClosed((before) => { const next = new Set(before); if (expanded) next.delete(id); else next.add(id); return next; })} />
    </div>}

    {source.isError && <ErrorState compact error={source.error} onRetry={retry}/>}
    {recent.error!==null && <ErrorState compact error={recent.error} onRetry={recent.retry}/>}
    {(index>0||more) && <div className="flex justify-end gap-2">
      <Button size="sm" disabled={index===0||waiting} onClick={()=>setPage({scope,index:index-1})}>{t('resourceDetails.layout.previous')}</Button>
      <Button size="sm" disabled={!more||waiting} pending={source.isFetchingNextPage} onClick={()=>void next()}>{t('resourceDetails.layout.next')}</Button>
    </div>}
  </Popover>}</Field>;
}
