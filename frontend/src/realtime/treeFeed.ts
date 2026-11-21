/** Open hierarchy pages share one source in the document metadata protocol per root. */
import { ApiError, client } from '@/api/client';
import { hierarchyFeed, readHierarchyFeed } from '@/api/hierarchyFeed';
import { fetchTreePage, splitListed, readTreeOrdering, type TreeReply, type TreeSelection } from '@/api/nodes';
import { record } from '@/api/cache';
import { followHierarchy } from '@/realtime/hierarchySource';
import { loadWithRetries } from '@/realtime/sync';
import { treeWriter } from '@/realtime/stores/tree';
import type { FeedFrame, RealtimeSocket, Subscription } from '@/realtime/socket';

interface Page {window:number;data:TreeReply;query:Record<string,unknown>;version:number;error:unknown}
interface Branch {
  org:string;parent:string;selection:TreeSelection;users:number;generation:number;
  reader:number|undefined;pages:Map<string,Page>;controller:AbortController|null;error:unknown;
}
interface Source {feed:string;subscription:Subscription|null}
const FIRST='';
export const hierarchySearchKey=(query:string)=>`search:${query}`;

function failure(code:string):ApiError {
  const denied=code==='authz.denied'||code==='forbidden'||code==='revoked';
  const missing=code==='not_found'||code==='deleted';
  const problem=denied?'authz.denied':missing?'not_found':'internal';
  return new ApiError({type:`orch:error:${problem}`,code:problem,title:'',detail:'',status:denied?403:missing?404:503,request_id:''},null);
}

const cursorBoundary=(cursor:string|null|undefined):string|null=>{if(!cursor)return null;const parts=cursor.split('|');if(parts.length===5)parts[2]='current';return parts.join('|');};

export function createHierarchyFeeds(socket:Pick<RealtimeSocket,'subscribe'>&Partial<Pick<RealtimeSocket,'status'>>, changed?:(org:string,rows:TreeReply['nodes'])=>void) {
  const branches=new Map<string,Branch>(),sources=new Map<string,Source>(),contexts=new Map<string,{selection:TreeSelection;signal:unknown}>();
  let nextWindow=1;
  const key=(org:string,parent:string)=>JSON.stringify([org,parent]);
  const current=(branch:Branch,generation:number)=>branch.users>0&&branch.generation===generation&&branch.reader===client.session.getState().readerVersion;
  const inOrg=(org:string)=>[...branches.values()].filter((branch)=>branch.org===org&&branch.users>0);

  function publish(branch:Branch):void {
    const pages=[...branch.pages.values()].map((page)=>page.data);
    if(pages.length===0){treeWriter.branchState(branch.org,branch.parent,branch.controller!==null,branch.error);return;}
    const rows=new Map(pages.flatMap((page)=>page.nodes.map((row)=>[row.id,row] as const)));
    const hidden=new Map(pages.flatMap((page)=>[...page.hidden,...page.ancestorHidden??[]].map((row)=>[row.id,row] as const)));
    const ancestors=new Map(pages.flatMap((page)=>(page.ancestors??[]).map((row)=>[row.id,row] as const)));
    changed?.(branch.org,[...ancestors.values(),...rows.values()]);
    treeWriter.branch(branch.org,branch.parent,[...rows.values()],pages.reduce((seq,page)=>Math.max(seq,page.seq),0),[...hidden.values()],[...ancestors.values()],pages.at(-1)?.next_cursor??null,pages.flatMap((page)=>page.ordering?[page.ordering]:[]));
    const partError=[...branch.pages.values()].find((page)=>page.error!==null)?.error;
    treeWriter.branchState(branch.org,branch.parent,branch.controller!==null,branch.error??partError??null);
  }

  function stopSource(org:string):void {const source=sources.get(org);sources.delete(org);source?.subscription?.unsubscribe();}
  function stopBranch(branch:Branch):void {
    branch.generation+=1;branch.controller?.abort();branch.controller=null;branch.pages.clear();
  }
  function resetOrg(org:string,reload:boolean,error:unknown=null):void {
    stopSource(org);const scoped=inOrg(org);
    for(const branch of scoped){stopBranch(branch);branch.error=error;}
    treeWriter.evict(org);
    if(reload){for(const branch of scoped)void read(branch);}
    else treeWriter.branchState(org,org,false,error);
  }

  function receive(org:string,source:Source,frame:FeedFrame):void {
    if(sources.get(org)!==source)return;
    if(frame.op==='resync'){if(frame.reload===true)resetOrg(org,true);else refresh(org);return;}
    if(frame.op==='closed'&&frame.reason!=='unsubscribed'||frame.op==='error'&&frame.code!=='too_many_subscriptions') {
      resetOrg(org,false,failure(frame.op==='closed'?frame.reason:frame.code));return;
    }
    if(frame.op!=='event'&&frame.op!=='sync')return;
    const value=frame.data.hierarchy,window=frame.data.window;
    if(!record(value)||!Array.isArray(value.nodes)||typeof value.seq!=='number'||typeof window!=='number')return;
    for(const branch of inOrg(org))for(const [cursor,page] of branch.pages) {
      if(branch.reader!==client.session.getState().readerVersion||page.window!==window)continue;
      const ancestry=splitListed(Array.isArray(value.ancestors)?value.ancestors:[]),rows=splitListed(value.nodes);
      const nextCursor=typeof value.next_cursor==='string'?value.next_cursor:null;
      if(cursorBoundary(page.data.next_cursor)!==cursorBoundary(nextCursor)) {
        let after=false;
        for(const position of branch.pages.keys()){if(after)branch.pages.delete(position);if(position===cursor)after=true;}
        if(branch.controller!==null){branch.controller.abort();branch.controller=null;queueMicrotask(()=>more(org,branch.parent));}
      }
      page.data={...page.data,...rows,ancestors:ancestry.nodes,ancestorHidden:ancestry.hidden,seq:value.seq,complete:value.complete===true,next_cursor:nextCursor,ordering:readTreeOrdering(value.ordering)};
      page.version+=1;page.error=typeof frame.data.problem==='string'?failure(frame.data.problem):null;
      branch.error=null;publish(branch);
    }
    syncSource(org);
  }

  function syncSource(org:string):void {
    const pages=inOrg(org).flatMap((branch)=>[...branch.pages.values()]);
    if(pages.length===0){stopSource(org);return;}
    const feed=hierarchyFeed();
    if(sources.get(org)?.feed===feed){sources.get(org)?.subscription?.updateViews?.();return;}
    stopSource(org);
    const source:Source={feed,subscription:null};sources.set(org,source);
    source.subscription=followHierarchy(socket,org,{since:()=>null,hierarchy:()=>inOrg(org).flatMap((branch)=>[...branch.pages.values()].map((page)=>({window:page.window,root:org,query:page.query}))),onFrame:(frame)=>receive(org,source,frame)});
  }

  async function read(branch:Branch,cursor=FIRST):Promise<void> {
    if(branch.controller!==null)return;
    const generation=branch.generation,controller=new AbortController(),held=branch.pages.get(cursor),version=held?.version;
    branch.controller=controller;branch.error=null;treeWriter.branchState(branch.org,branch.parent,true,null);
    try {
      const selected={...contexts.get(branch.org)?.selection,...branch.selection,...(cursor===FIRST?{}:{cursor})};
      const data=await loadWithRetries(()=>fetchTreePage(branch.org,selected,controller.signal),controller.signal);
      if(!current(branch,generation)||branch.controller!==controller||held!==undefined&&(branch.pages.get(cursor)!==held||held.version!==version))return;
      const locator=data.feed?readHierarchyFeed(data.feed):null;
      if(locator===null)throw failure('internal');
      if(held!==undefined&&cursorBoundary(held.data.next_cursor)!==cursorBoundary(data.next_cursor)){
        let later=false;for(const key of branch.pages.keys()){if(later)branch.pages.delete(key);if(key===cursor)later=true;}
      }
      branch.pages.set(cursor,{window:held?.window??nextWindow++,data,query:selected,version:0,error:null});publish(branch);syncSource(branch.org);
    } catch(error) {
      if(!current(branch,generation)||controller.signal.aborted)return;
      branch.error=error;
      if(error instanceof ApiError&&(error.is('authz.denied')||error.is('not_found'))){branch.pages.clear();treeWriter.releaseBranch(branch.org,branch.parent);syncSource(branch.org);}
    } finally {
      if(current(branch,generation)&&branch.controller===controller){branch.controller=null;publish(branch);}
    }
  }

  function configure(org:string,selection:TreeSelection,signal?:unknown):void {
    const before=contexts.get(org);const nextSignal=signal??before?.signal;
    contexts.set(org,{selection,signal:nextSignal});
    // The first report describes the native order already requested by the warm reader.
    if(before!==undefined && JSON.stringify(before.selection)===JSON.stringify(selection)
      && (before.signal===undefined || before.signal===nextSignal))return;
    stopSource(org);
    for(const branch of inOrg(org)){stopBranch(branch);branch.error=null;void read(branch);}
  }
  function watch(org:string,parent:string,selection:TreeSelection={parent_id:parent}):()=>void {
    const id=key(org,parent);let branch=branches.get(id);
    if(branch===undefined){branch={org,parent,selection,users:0,generation:0,reader:client.session.getState().readerVersion,pages:new Map(),controller:null,error:null};branches.set(id,branch);}
    branch.users+=1;if(branch.users===1)void read(branch);
    const held=branch;let active=true;
    return()=>{if(!active)return;active=false;held.users-=1;if(held.users>0)return;stopBranch(held);branches.delete(id);syncSource(org);if(inOrg(org).length===0)contexts.delete(org);};
  }
  function more(org:string,parent:string):void {
    const branch=branches.get(key(org,parent));if(branch===undefined)return;
    const last=[...branch.pages.values()].at(-1);
    if(branch.error!==null||last?.error!==null&&last?.error!==undefined){void read(branch,[...branch.pages.keys()].at(-1)??FIRST);return;}
    if(last?.data.next_cursor)void read(branch,last.data.next_cursor);
  }
  function refresh(org:string):void {
    for(const branch of inOrg(org)){if(branch.pages.size===0)void read(branch);else for(const cursor of branch.pages.keys())void read(branch,cursor);}
  }
  const stopStatus=socket.status?.subscribe((next,previous)=>{
    if(next.phase==='ready'&&previous.phase!=='ready')for(const org of new Set([...branches.values()].filter((branch)=>branch.error!==null).map((branch)=>branch.org)))refresh(org);
  });
  const stopReader=client.session.subscribe((next,previous)=>{
    if(next.readerVersion===previous.readerVersion)return;
    const orgs=new Set([...branches.values()].map((branch)=>branch.org));
    for(const org of orgs)stopSource(org);treeWriter.clear();
    for (const branch of branches.values()) {
      stopBranch(branch);
      branch.reader = next.readerVersion;
      branch.error = null;
      if (next.status === 'signed_in') {
        // Let the reader's session listeners release hidden trees before opening any remaining branch.
        const generation = branch.generation;
        queueMicrotask(() => {
          if (current(branch, generation) && branch.pages.size === 0) void read(branch);
        });
      }
    }
    if(next.status==='signed_out')branches.clear();
  });
  const dispose=()=>{stopStatus?.();stopReader();for(const org of sources.keys())stopSource(org);for(const branch of branches.values())stopBranch(branch);branches.clear();contexts.clear();};
  return {watch,more,refresh,configure,dispose};
}
