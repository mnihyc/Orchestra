/** Place lists retain one native page per root, with server search and the document metadata feed. */
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { ApiError, client } from '@/api/client';
import { record } from '@/api/cache';
import { fetchTreePage, splitListed, treePageKey, type TreeReply, type TreeSelection } from '@/api/nodes';
import type { TreeNode } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import type { Subscription } from '@/realtime/socket';
import { Button } from '@/ui/Button';

const EMPTY_PATHS:ReadonlyMap<string,readonly string[]>=new Map();

export function usePlacePages(roots:readonly {id:string;name:string}[], selection:TreeSelection, active:boolean) {
  const cache=useQueryClient(),reader=useStore(client.session,(state)=>state.readerVersion);
  const scope=JSON.stringify([reader,roots.map((root)=>root.id),selection]);
  const [position,setPosition]=useState<{scope:string;paths:ReadonlyMap<string,readonly string[]>}>({scope,paths:new Map()});
  const paths=position.scope===scope?position.paths:EMPTY_PATHS;
  const requests=useMemo(()=>active?roots.map((root)=>{
    const cursor=paths.get(root.id)?.at(-1), selected={...selection,...(cursor?{cursor}:{})};
    return {root,key:treePageKey(root.id,selected,reader),selected};
  }):[],[roots,selection,paths,active,reader]);
  const results=useQueries({queries:requests.map((request)=>({queryKey:request.key,
    queryFn:({signal}:{signal:AbortSignal})=>fetchTreePage(request.root.id,request.selected,signal),retry:false,gcTime:0})),
    combine:(rows)=>rows.map((row)=>({data:row.data,error:row.error,isPending:row.isPending,isFetching:row.isFetching,isError:row.isError,refetch:row.refetch}))});
  const sourceKey=JSON.stringify(requests.flatMap((request,index)=>{const feed=results[index]?.data?.feed;return feed?[{key:request.key,feed}]:[];}));
  useEffect(()=>{
    if(!active)return;
    let current=true;
    const sources=JSON.parse(sourceKey) as {key:readonly unknown[];feed:string}[];
    const subscriptions:Subscription[]=sources.map((source)=>socket.subscribe(source.feed,{since:()=>null,onFrame:(frame)=>{
      if(!current || reader!==client.session.getState().readerVersion)return;
      const query=cache.getQueryCache().find({queryKey:source.key,exact:true});
      if(!query?.isActive())return;
      if(frame.op==='event'||frame.op==='sync') {
        const value=frame.data.hierarchy;
        if(!record(value)||!Array.isArray(value.nodes)||typeof value.seq!=='number')return;
        if(typeof frame.data.problem==='string') {
          const native=frame.data.problem,denied=native.startsWith('authz.'),missing=native==='not_found';
          const code=denied?'authz.denied':missing?'not_found':native==='request.invalid'?'request.invalid':'internal';
          const error=new ApiError({type:`orch:error:${code}`,code,title:'',detail:'',status:denied?403:missing?404:code==='request.invalid'?400:503,request_id:''},null);
          void query.cancel({revert:false,silent:true});query.setState({data:undefined,error,status:'error',fetchStatus:'idle'});return;
        }
        const ancestry=splitListed(Array.isArray(value.ancestors)?value.ancestors:[]);
        const rows=splitListed(value.nodes),seq=value.seq,complete=value.complete===true,next_cursor=typeof value.next_cursor==='string'?value.next_cursor:null;
        void query.cancel({revert:false,silent:true});
        cache.setQueryData<TreeReply>(source.key,(held)=>held===undefined?undefined:{...held,...rows,ancestors:ancestry.nodes,ancestorHidden:ancestry.hidden,seq,complete,next_cursor});
      } else if(frame.op==='resync') {
        void query.cancel({revert:false,silent:true});
        if(frame.reload===true)query.setState({data:undefined,dataUpdatedAt:0,error:null,status:'pending',fetchStatus:'idle'});
        void cache.invalidateQueries({queryKey:source.key,exact:true});
      }
      else if(frame.op==='closed' && frame.reason!=='unsubscribed' || frame.op==='error' && frame.code!=='too_many_subscriptions') {
        const denied=frame.op==='closed'?frame.reason==='revoked':frame.code==='forbidden';
        const code=denied?'authz.denied':'not_found';
        const error=new ApiError({type:`orch:error:${code}`,code,title:'',detail:'',status:denied?403:404,request_id:''},null);
        void query.cancel({revert:false,silent:true});query.setState({data:undefined,error,status:'error',fetchStatus:'idle'});
      }
    }}));
    return()=>{current=false;for(const subscription of subscriptions)subscription.unsubscribe();};
  },[cache,active,reader,sourceKey]);
  const step=useCallback((root:string,next:string|null)=>setPosition((before)=>{
    const paths=new Map(before.scope===scope?before.paths:[]),old=paths.get(root)??[];
    paths.set(root,next===null?old.slice(0,-1):[...old,next]);return {scope,paths};
  }),[scope]);
  const pages=useMemo(()=>results.map((result,index)=>{
    const request=requests[index];
    if(request===undefined)throw new Error('A place page needs its requested root');
    const root=request.root;
    const rows=new Map<string,TreeNode>([...result.data?.ancestors??[],...result.data?.nodes??[]].map((row)=>[row.id,row]));
    return {root,result,rows:[...rows.values()],matches:new Set(result.data?.nodes.map((row)=>row.id)),
      back:(paths.get(root.id)?.length??0)>0,previous:()=>step(root.id,null),next:()=>{const cursor=result.data?.next_cursor;if(cursor)step(root.id,cursor);}};
  }),[results,requests,paths,step]);
  return {pages,pending:active&&results.some((result)=>result.isPending),error:results.find((result)=>result.isError)?.error??null,
    retry:()=>{for(const result of results)if(result.isError)void result.refetch();}};
}

/** Existing picker footers keep paging controls beside the list they affect. */
export function PlacePageFoot({pages}:{pages:ReturnType<typeof usePlacePages>['pages']}) {
  const {t}=useTranslation();
  return <>{pages.filter((page)=>page.back||Boolean(page.result.data?.next_cursor)).map((page)=><div key={page.root.id} className="flex min-w-0 items-center justify-end gap-2">
    {pages.length>1&&<span className="min-w-0 flex-1 truncate text-xs text-text-muted">{page.root.name}</span>}
    {page.back&&<Button size="sm" variant="ghost" onClick={page.previous} disabled={page.result.isFetching}>{t('navigation.pages.previous')}</Button>}
    {page.result.data?.next_cursor&&<Button size="sm" variant="ghost" onClick={page.next} pending={page.result.isFetching}>{t('navigation.pages.next')}</Button>}
  </div>)}</>;
}
