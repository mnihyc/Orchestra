/** Pending index work and setup dialogs belong to the page, independently of its displayed rows. */
import {createContext,useCallback,useContext,useMemo,useState,type ReactNode} from 'react';
import {useQueryClient,type InfiniteData} from '@tanstack/react-query';
import {useTranslation} from 'react-i18next';
import {client} from '@/api/client';
import {keepSemanticSearch,rebuildSemanticSearch,semanticSearchKey} from '@/api/admin';
import {settingsKey} from '@/api/settings';
import type {SemanticSearchOrg,SemanticSearchOverview} from '@/generated/api-types';
import {AddProviderDialog} from '@/features/resources/providers/AddProvider';
import {SettingsEditor} from '@/features/settings/Editor';
import {Dialog} from '@/ui/Dialog';
import {Button} from '@/ui/Button';
import {ErrorState} from '@/ui/ErrorState';

interface Work {org:SemanticSearchOrg;adding:boolean;settings:boolean;pending:'rebuild'|'keep'|null;error:unknown}
interface Controls {work:ReadonlyMap<string,Work>;open:(org:SemanticSearchOrg,kind:'adding'|'settings',open:boolean)=>void;run:(org:SemanticSearchOrg,kind:'rebuild'|'keep')=>void}
const Context=createContext<Controls|null>(null);
export function useSemanticActions(org:SemanticSearchOrg) {
 const controls=useContext(Context);if(controls===null)throw new Error('Semantic actions need their page owner.');
 const work=controls.work.get(org.org.id);
 return {adding:work?.adding??false,settings:work?.settings??false,setAdding:(open:boolean)=>controls.open(org,'adding',open),setSettings:(open:boolean)=>controls.open(org,'settings',open),
  rebuild:{isPending:work?.pending==='rebuild',mutate:()=>controls.run(org,'rebuild'),error:work?.error??null},
  keep:{isPending:work?.pending==='keep',mutate:()=>controls.run(org,'keep'),error:null},
 };
}
export function SemanticActionsOwner({children}:{children:ReactNode}) {
 const {t}=useTranslation();const cache=useQueryClient();const [work,setWork]=useState<ReadonlyMap<string,Work>>(new Map());
 const change=useCallback((org:SemanticSearchOrg,patch:Partial<Omit<Work,'org'>>)=>setWork((before)=>{
  const next=new Map(before);const item={adding:false,settings:false,pending:null,error:null,...before.get(org.org.id),org,...patch};
  if(!item.adding&&!item.settings&&item.pending===null&&item.error===null)next.delete(org.org.id);else next.set(org.org.id,item);return next;
 }),[]);
 const open=useCallback((org:SemanticSearchOrg,kind:'adding'|'settings',value:boolean)=>change(org,{[kind]:value}),[change]);
 const run=useCallback((org:SemanticSearchOrg,kind:'rebuild'|'keep')=>{
  if(work.get(org.org.id)?.pending!==null&&work.get(org.org.id)?.pending!==undefined)return;
  const reader=client.session.getState();const version=reader.readerVersion,person=reader.userId;
  const current=()=>client.session.getState().readerVersion===version&&client.session.getState().userId===person;
  change(org,{pending:kind,error:null});
  const request=kind==='rebuild'?rebuildSemanticSearch(org.org.id):org.active===null?Promise.reject(new Error(t('admin.semantic.noActive'))):keepSemanticSearch(org.org.id,org.active.id);
  void request.then((row)=>{
   if(!current())return;
   cache.setQueryData<SemanticSearchOverview>(semanticSearchKey,(before)=>before===undefined?before:{...before,orgs:before.orgs.map((old)=>old.org.id===row.org.id?row:old)});
   cache.setQueriesData<InfiniteData<SemanticSearchOverview>>({queryKey:[...semanticSearchKey,'pages']},(before)=>before===undefined?before:{...before,pages:before.pages.map((page)=>({...page,orgs:page.orgs.map((old)=>old.org.id===row.org.id?row:old)}))});
   change(row,{pending:null,error:null});void cache.invalidateQueries({queryKey:settingsKey(row.org.id),exact:true});
  }).catch((error:unknown)=>{if(current())change(org,{pending:null,error});});
 },[work,change,cache,t]);
 const value=useMemo(()=>({work,open,run}),[work,open,run]);
 return <Context value={value}>{children}{[...work.values()].map((state)=><div key={state.org.org.id}>
  {state.adding&&<AddProviderDialog initialHome={state.org.org.id} initialPreset="Local embeddings" onClose={()=>open(state.org,'adding',false)} onCreated={()=>change(state.org,{adding:false,settings:true})}/>}
  {state.settings&&<Dialog open title={t('admin.semantic.embeddings')} onOpenChange={(value)=>open(state.org,'settings',value)} footer={<Button onClick={()=>open(state.org,'settings',false)}>{t('common.close')}</Button>}>
   <SettingsEditor target={{kind:'node',id:state.org.org.id}} scope="org" only={['search.semantic.embeddings']} compact place={{name:state.org.org.name??'',owner:null,access:null,kind:'org'}}/>
  </Dialog>}
  {state.error!==null&&<ErrorState compact title={state.org.org.name??t('ui.nameUnknown')} error={state.error} action={<Button size="sm" onClick={()=>change(state.org,{error:null})}>{t('common.close')}</Button>}/>}
 </div>)}</Context>;
}
