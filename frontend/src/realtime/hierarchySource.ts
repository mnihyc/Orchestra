/** All active hierarchy pages share the existing metadata subscription, across roots and selectors. */
import { client } from '@/api/client';
import { hierarchyFeed } from '@/api/hierarchyFeed';
import type { FeedFrame, FeedListener, HierarchyPage, RealtimeSocket, Subscription } from './socket';

interface Reader {
  root:string;
  listener:FeedListener;
  windows:Map<number,number>;
}
interface Source {
  reader:number|undefined;
  readers:Set<Reader>;
  subscription:Subscription|null;
  control:FeedFrame|null;
  nextWindow:number;
}
const sources=new WeakMap<Pick<RealtimeSocket,'subscribe'>,Source>();

export function followHierarchy(socket:Pick<RealtimeSocket,'subscribe'>,root:string,listener:FeedListener):Subscription {
  const version=client.session.getState().readerVersion;
  let source=sources.get(socket);
  if(source!==undefined&&source.reader!==version){source.readers.clear();source.subscription?.unsubscribe();source.subscription=null;sources.delete(socket);source=undefined;}
  if(source===undefined){source={reader:version,readers:new Set(),subscription:null,control:null,nextWindow:1};sources.set(socket,source);}
  const held=source,reader:Reader={root,listener,windows:new Map()};held.readers.add(reader);
  const pages=():HierarchyPage[]=>[...held.readers].flatMap((reader)=>{
    const wanted=reader.listener.hierarchy?.()??[],ids=new Set(wanted.map((page)=>page.window));
    for(const id of reader.windows.keys())if(!ids.has(id))reader.windows.delete(id);
    return wanted.map((page)=>{
      let window=reader.windows.get(page.window);
      if(window===undefined){window=held.nextWindow++;reader.windows.set(page.window,window);}
      return {window,root:reader.root,query:page.query};
    });
  });
  if(held.subscription===null) {
    held.subscription=socket.subscribe(hierarchyFeed(),{since:()=>null,hierarchy:pages,onFrame:(frame)=>{
      if(sources.get(socket)!==held||held.reader!==client.session.getState().readerVersion)return;
      if(frame.op==='subscribed'||frame.op==='error'&&frame.code==='too_many_subscriptions')held.control=frame;
      else if(frame.op==='resync'||frame.op==='closed'||frame.op==='error')held.control=null;
      if((frame.op==='event'||frame.op==='sync')&&typeof frame.data.window==='number') {
        for(const reader of held.readers)for(const [local,window]of reader.windows)if(window===frame.data.window) {
          reader.listener.onFrame({...frame,data:{...frame.data,window:local}});return;
        }
        return;
      }
      for(const reader of [...held.readers])if(held.readers.has(reader))reader.listener.onFrame(frame);
    }});
  } else {
    // A new logical view gets the actual source state; page data still needs its own native read.
    const control=held.control;
    if(control!==null)queueMicrotask(()=>{if(held.readers.has(reader))reader.listener.onFrame(control);});
    held.subscription.updateViews?.();
  }
  let active=true;
  return {
    updateViews:()=>{if(active)held.subscription?.updateViews?.();},
    unsubscribe:()=>{
      if(!active)return;active=false;held.readers.delete(reader);
      if(held.readers.size===0){if(sources.get(socket)===held)sources.delete(socket);held.subscription?.unsubscribe();held.subscription=null;}
      else held.subscription?.updateViews?.();
    },
  };
}
