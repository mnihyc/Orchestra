// @vitest-environment node
/** A selector's roots share metadata demand without taking the socket away from conversation feeds. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { hierarchyFeed } from '@/api/hierarchyFeed';
import { createHierarchyFeeds } from '@/realtime/treeFeed';
import { createRealtimeSocket, type SocketLike, type FeedListener, type RealtimeSocket } from '@/realtime/socket';
import { treeWriter } from '@/realtime/stores/tree';
import { listedTreeNode } from '../doubles/tree-node';

const reads=vi.hoisted(()=>({get:vi.fn()}));
vi.mock('@/api/client',async(load)=>({...await load<object>(),api:{GET:reads.get}}));
const initial=client.session.getState();
const cleanups:(()=>void)[]=[];

beforeEach(()=>{reads.get.mockReset();client.session.setState({status:'signed_in',userId:'reader',readerVersion:(client.session.getState().readerVersion??0)+1});});
afterEach(()=>{for(const close of cleanups.splice(0))close();treeWriter.clear();client.session.setState(initial);});

it('a hundred roots share one admitted metadata source beside the inbox and a conversation',async()=>{
  const sent:Record<string,unknown>[]=[];
  const wire:SocketLike={readyState:1,onopen:null,onmessage:null,onclose:null,onerror:null,
    close:()=>undefined,send:(text)=>{
      const frame:unknown=JSON.parse(text);if(typeof frame!=='object'||frame===null)return;
      sent.push(frame as Record<string,unknown>);
      if('op' in frame&&frame.op==='sub'&&'id' in frame&&'feed' in frame&&!('window' in frame))queueMicrotask(()=>wire.onmessage?.({data:JSON.stringify({op:'subscribed',id:frame.id,feed:frame.feed,seq:null,mode:'ephemeral'})}));
    }};
  const socket=createRealtimeSocket({token:()=> 'token',renew:()=>Promise.resolve(true),onSuspended:()=>undefined,connect:()=>wire,url:()=> 'ws://orch.test/ws'});
  cleanups.push(socket.stop);socket.start();wire.onopen?.();wire.onmessage?.({data:JSON.stringify({op:'ready',connection_id:'c',user_id:'reader',server_time:'',heartbeat_secs:25,access_expires_at:null,build:'b',max_client_frame_bytes:16*1024})});
  socket.subscribe('user',{since:()=>null,onFrame:()=>undefined});
  socket.subscribe('thread:current',{since:()=>null,onFrame:()=>undefined});
  reads.get.mockImplementation((_path:unknown,{params}:{params:{path:{id:string}}})=>Promise.resolve({response:new Response(null,{status:200}),data:{seq:1,since:null,complete:true,next_cursor:null,feed:hierarchyFeed(),ancestors:[],nodes:[listedTreeNode({id:`place-${params.path.id}`,kind:'project',parent_id:params.path.id})]}}));
  const feeds=createHierarchyFeeds(socket),other=createHierarchyFeeds(socket);cleanups.push(feeds.dispose,other.dispose);
  const stops=Array.from({length:100},(_,index)=>(index%2===0?feeds:other).watch(`root-${index}`,`root-${index}`));
  await vi.waitFor(()=>expect(reads.get).toHaveBeenCalledTimes(100));
  await vi.waitFor(()=>expect(sent.filter((frame)=>frame.op==='sub'&&typeof frame.window==='object')).toHaveLength(100));
  const admissions=sent.filter((frame)=>frame.op==='sub'&&frame.window===undefined);
  expect(admissions.map((frame)=>frame.feed)).toHaveLength(3);
  const metadata=admissions.find((frame)=>typeof frame.feed==='string'&&frame.feed.startsWith('document-tree:'));
  expect(metadata).toBeDefined();
  for(const frame of sent)expect(new TextEncoder().encode(JSON.stringify(frame)).length).toBeLessThanOrEqual(16*1024);
  for(const stop of stops)stop();
  expect(sent.filter((frame)=>frame.op==='unsub')).toEqual([{op:'unsub',id:metadata?.id}]);
});

it('a new logical page never replays an earlier resync as its own opening',async()=>{
  const {followHierarchy}=await import('@/realtime/hierarchySource');
  const listeners:FeedListener[]=[],first=vi.fn(),next=vi.fn();
  const socket={subscribe:vi.fn<RealtimeSocket['subscribe']>((_feed,listener)=>{listeners.push(listener);return {unsubscribe:()=>undefined,updateViews:()=>undefined};})};
  const one=followHierarchy(socket,'one',{since:()=>null,hierarchy:()=>[],onFrame:first});cleanups.push(one.unsubscribe);
  listeners[0]?.onFrame({op:'resync',id:1,feed:hierarchyFeed(),reload:true});
  const two=followHierarchy(socket,'two',{since:()=>null,hierarchy:()=>[],onFrame:next});cleanups.push(two.unsubscribe);
  await Promise.resolve();
  expect(first).toHaveBeenCalledTimes(1);expect(next).not.toHaveBeenCalled();
});

it('a replacement reader gets a new metadata source before old selector effects finish leaving',async()=>{
  const {followHierarchy}=await import('@/realtime/hierarchySource');
  const listeners:FeedListener[]=[],unsubscribers=vi.fn(),first=vi.fn(),next=vi.fn();
  const socket={subscribe:vi.fn<RealtimeSocket['subscribe']>((_feed,listener)=>{listeners.push(listener);return {unsubscribe:unsubscribers,updateViews:()=>undefined};})};
  const old=followHierarchy(socket,'one',{since:()=>null,hierarchy:()=>[],onFrame:first});cleanups.push(old.unsubscribe);
  client.session.setState({readerVersion:(client.session.getState().readerVersion??0)+1,userId:'replacement'});
  const current=followHierarchy(socket,'two',{since:()=>null,hierarchy:()=>[],onFrame:next});cleanups.push(current.unsubscribe);
  expect(socket.subscribe).toHaveBeenCalledTimes(2);expect(unsubscribers).toHaveBeenCalledTimes(1);
  old.unsubscribe();expect(unsubscribers).toHaveBeenCalledTimes(1);
  listeners[0]?.onFrame({op:'event',id:1,feed:hierarchyFeed(),data:{window:1,hierarchy:{seq:1,nodes:[]}}});
  expect(first).not.toHaveBeenCalled();expect(next).not.toHaveBeenCalled();
});
