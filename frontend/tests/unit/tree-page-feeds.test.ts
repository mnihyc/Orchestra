/** A bounded native page owns one read, one metadata subscription and one reader. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { hierarchyFeed } from '@/api/hierarchyFeed';
import { client } from '@/api/client';
import { createHierarchyFeeds } from '@/realtime/treeFeed';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import type { FeedListener, RealtimeSocket } from '@/realtime/socket';
import type { OrgTreeGetReply } from '@/generated/api-types';
import { listedTreeNode } from '../doubles/tree-node';

interface Reply {data:OrgTreeGetReply;response:Response}
const calls=vi.hoisted(()=>({get:vi.fn<()=>Promise<Reply>>()}));
vi.mock('@/api/client',async(load)=>({...await load<object>(),api:{GET:calls.get}}));
const initial=client.session.getState();
const cleanups:(()=>void)[]=[];
const root=listedTreeNode({id:'root',kind:'org',parent_id:null,folded:{has_children:true,small_tree:true,unread:false,waiting:0,mentions:0,urgency:null}});
const project=listedTreeNode({id:'project',kind:'project',parent_id:'root',tree_seq:1});
function reply(name='Original',seq=1):Reply {return {data:{seq,since:null,complete:true,feed:hierarchyFeed(),page_size:50,ancestors:[root],nodes:[{...project,name,tree_seq:seq}]},response:new Response(null,{status:200})};}
function setup() {
  const listeners:FeedListener[]=[],unsub=vi.fn();
  const subscribe=vi.fn<RealtimeSocket['subscribe']>((_feed,listener)=>{listeners.push(listener);return {unsubscribe:unsub};});
  const feeds=createHierarchyFeeds({subscribe});cleanups.push(feeds.dispose);
  return {feeds,listeners,subscribe,unsub};
}
const nodes=()=>useTreeStore.getState().byOrg.get('root')?.nodes;
beforeEach(()=>{calls.get.mockReset();client.session.setState({status:'signed_in',userId:'first-person',readerVersion:(client.session.getState().readerVersion??0)+1});});
afterEach(()=>{for(const cleanup of cleanups.splice(0))cleanup();treeWriter.clear();client.session.setState(initial);});

it('two readers of one open branch share one bounded read and one metadata subscription',async()=>{
  calls.get.mockResolvedValue(reply());const live=setup();
  const first=live.feeds.watch('root','root'),second=live.feeds.watch('root','root');
  await vi.waitFor(()=>expect(live.subscribe).toHaveBeenCalledTimes(1));
  expect(calls.get).toHaveBeenCalledTimes(1);
  first();expect(live.unsub).not.toHaveBeenCalled();second();expect(live.unsub).toHaveBeenCalledTimes(1);
});

it('a retired branch cannot restore rows from its late native answer',async()=>{
  let finish!:(value:Reply)=>void;calls.get.mockReturnValue(new Promise((resolve)=>{finish=resolve;}));
  const live=setup(),release=live.feeds.watch('root','root');release();treeWriter.evict('root');
  finish(reply());await Promise.resolve();await Promise.resolve();
  expect(nodes()).toBeUndefined();expect(live.subscribe).not.toHaveBeenCalled();
});

it('replacing the reader retires its pending answer and starts a current native opening',async()=>{
  const pending:((value:Reply)=>void)[]=[];calls.get.mockImplementation(()=>new Promise((resolve)=>{pending.push(resolve);}));
  const live=setup();live.feeds.watch('root','root');
  client.session.setState({status:'signed_in',userId:'second-person',readerVersion:(client.session.getState().readerVersion??0)+1});
  expect(pending).toHaveLength(2);pending[1]?.(reply('Second reader',2));
  await vi.waitFor(()=>expect(nodes()?.get('project')?.name).toBe('Second reader'));
  pending[0]?.(reply('First reader',1));await Promise.resolve();await Promise.resolve();
  expect(nodes()?.get('project')?.name).toBe('Second reader');
});

it('a late HTTP repair cannot replace a newer live page',async()=>{
  calls.get.mockResolvedValueOnce(reply());const live=setup();live.feeds.watch('root','root');
  await vi.waitFor(()=>expect(live.listeners).toHaveLength(1));
  let finish!:(value:Reply)=>void;calls.get.mockReturnValueOnce(new Promise((resolve)=>{finish=resolve;}));
  live.feeds.refresh('root');
  const listener=live.listeners[0];if(listener===undefined)throw new Error('The metadata source must be open');
  listener.onFrame({op:'event',id:1,feed:hierarchyFeed(),data:{window:listener.hierarchy?.()[0]?.window,hierarchy:reply('Live',2).data}});
  finish(reply('Older repair',1));await Promise.resolve();await Promise.resolve();
  expect(nodes()?.get('project')?.name).toBe('Live');expect(live.subscribe).toHaveBeenCalledTimes(1);
});

it('an authority resync clears the old projection and cancels its in-flight repair before reopening',async()=>{
  calls.get.mockResolvedValueOnce(reply());const live=setup();live.feeds.watch('root','root');
  await vi.waitFor(()=>expect(live.listeners).toHaveLength(1));
  let finish!:(value:Reply)=>void;calls.get.mockReturnValueOnce(new Promise((resolve)=>{finish=resolve;})).mockResolvedValueOnce(reply('Current authority',3));
  live.feeds.refresh('root');live.listeners[0]?.onFrame({op:'resync',id:1,feed:hierarchyFeed(),reload:true});
  expect(nodes()?.size??0).toBe(0);
  await vi.waitFor(()=>expect(nodes()?.get('project')?.name).toBe('Current authority'));
  finish(reply('Retired authority',2));await Promise.resolve();await Promise.resolve();
  expect(nodes()?.get('project')?.name).toBe('Current authority');
});

it('retrying an initial failure reads its first page again without requiring another view',async()=>{
  calls.get.mockRejectedValueOnce(new Error('Offline')).mockResolvedValueOnce(reply());
  const live=setup();live.feeds.watch('root','root');
  await vi.waitFor(()=>expect(useTreeStore.getState().byOrg.get('root')?.error).toBeInstanceOf(Error));
  live.feeds.refresh('root');await vi.waitFor(()=>expect(nodes()?.has('project')).toBe(true));
  expect(calls.get).toHaveBeenCalledTimes(2);
});

it('sixty-five open branches use one live source without filling the sockets subscription limit',async()=>{
  calls.get.mockImplementation(()=>{
    return Promise.resolve(reply());
  });
  const active=new Set<string>();
  const subscribe=vi.fn<RealtimeSocket['subscribe']>((feed)=>{active.add(feed);expect(active.size).toBe(1);return {unsubscribe:()=>{active.delete(feed);}};});
  const feeds=createHierarchyFeeds({subscribe});cleanups.push(feeds.dispose);
  const releases=Array.from({length:65},(_,index)=>feeds.watch('root',`branch-${index}`));
  await vi.waitFor(()=>expect(calls.get).toHaveBeenCalledTimes(65));
  await vi.waitFor(()=>expect(active.size).toBe(1));
  for(const release of releases)release();
  expect(active.size).toBe(0);
});
