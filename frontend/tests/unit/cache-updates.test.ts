/** Act answers and live rows replace loaded projections without refetching unrelated resources or any lists. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { describe, expect, it, vi } from 'vitest';
import { applyChanges, findAct, resourceChanges } from '@/api/acts';
import { applyRow, cachedRow } from '@/api/cache';
import { LiveQueryClient, queryClient } from '@/api/query';
import { createApiClient, type Act } from '@/api/client';
import { targetSettingsKey } from '@/api/settings';
import { environmentSettingsKey } from '@/api/environments';
import { providerSettingsKey } from '@/api/resources';
import { jobQuery as forkJob } from '@/api/forks';
import { jobQuery as exportJob } from '@/api/exports';
import { retentionJobQuery } from '@/api/trace';
import { answerChanges } from '@/api/actAnswers';
import type { Environment } from '@/generated/api-types';

const before = { id: 'p1', name: 'Team provider', status: 'active', home: { id: 'o1', name: 'Acme' }, access: { permissions: ['provider.view', 'provider.use'], administers: false }, revision: 1 };
const after = { ...before, name: 'Research provider', revision: 2 };

describe('answers replace rows', () => {
  it('replaces the successful empty catalogue read with the upload answer without reloading usage',async()=>{
    const cache=new QueryClient();cache.setQueryData(['usage','catalogue',null],null);
    cache.setQueryData(['usage','report','me'],{turns:19});
    const read=vi.fn(()=>Promise.resolve(null));
    const observer=new QueryObserver(cache,{queryKey:['usage','catalogue',null],queryFn:read,staleTime:Infinity});
    const off=observer.subscribe(()=>undefined);
    const catalogue={format:1,as_of:'2026-10-15',unit:'USD',families:[]};
    try {
      const changes=findAct('POST','/admin/usage/catalogue',{answer:catalogue})?.changes??[];
      expect(changes).toEqual([{value:{key:['usage','catalogue',null],data:catalogue}},{rateViews:{nodeId:null}}]);
      await applyChanges(changes,cache);
      expect(cache.getQueryData(['usage','catalogue',null])).toEqual(catalogue);
      expect(cache.getQueryData(['usage','report','me'])).toEqual({turns:19});
      expect(read).not.toHaveBeenCalled();
    } finally {off();cache.clear();}
  });

  it('does not mistake a named nested program for the machine that returned it',async()=>{
    const cache=new QueryClient();cache.setQueryData(['machine','m1'],{id:'m1',name:'Workstation',platform_os:'linux'});
    const changes=answerChanges({method:'POST',path:'/machines/m1/programs',answer:{id:'program-1',name:'Local coder'}},()=> 'm1',[{key:['machine'],id:'m1'}]);
    expect(changes.filter((change)=>'row' in change && change.row.kind==='machine')).toEqual([]);
    await applyChanges(changes,cache);
    expect(cache.getQueryData(['machine','program-1'])).toBeUndefined();cache.clear();
  });

  it('Keep applies its returned lasting environment to the detail and loaded list without a refetch',async()=>{
    const access:Environment['access']={permissions:['environment.view','environment.manage'],administers:false,participating:true,admin_only:[],through:[]};
    const cache=new QueryClient();const source={id:'e1',name:'Disposable',revision:1,temporary:{expires_at:'2026-10-18T12:00:00Z'},config:{'env.lifetime':'temporary'},access};
    cache.setQueryData(['environment','e1'],source);cache.setQueryData(['environments',null],[source]);
    const kept={...source,revision:2,temporary:null,config:{'env.lifetime':'lasting'}};
    const refresh=vi.spyOn(cache,'invalidateQueries');
    const refetch=vi.spyOn(cache,'refetchQueries');
    const read=vi.spyOn(cache,'query');
    await applyChanges(findAct('POST','/environments/e1/keep',{answer:kept})?.changes ?? [],cache);
    expect(cache.getQueryData(['environment','e1'])).toEqual(kept);
    expect(cache.getQueryData(['environments',null])).toEqual([kept]);
    expect(refresh).not.toHaveBeenCalled();expect(refetch).not.toHaveBeenCalled();expect(read).not.toHaveBeenCalled();cache.clear();
  });

  it('workspace Export applies the job answer without replacing the environment with a job',async()=>{
    const cache=new QueryClient();const environment={id:'e1',name:'Disposable'};
    cache.setQueryData(['environment','e1'],environment);
    const job={id:'j1',kind:'snapshot',state:'running',progress:{percent:0},result:null,batch_id:null,environment_id:'e1'};
    const refresh=vi.spyOn(cache,'invalidateQueries');
    const refetch=vi.spyOn(cache,'refetchQueries');
    const read=vi.spyOn(cache,'query');
    await applyChanges(findAct('POST','/environments/e1/files/export',{answer:job})?.changes ?? [],cache);
    expect(cache.getQueryData(['job','j1'])).toEqual(job);
    expect(cache.getQueryData(['environment','e1'])).toEqual(environment);
    expect(refresh).not.toHaveBeenCalled();expect(refetch).not.toHaveBeenCalled();expect(read).not.toHaveBeenCalled();cache.clear();
  });
  it('updates a person wherever already named while keeping private Me fields out of public references',async()=>{
    const cache=new QueryClient();const person={id:'u1',display_name:'Ada',avatar:{text:'A',color:1}};
    cache.setQueryData(['shell-bootstrap','u1'],{person,state:{sidebar:{}}});
    cache.setQueryData(['provider','p1'],{...before,owner:person});
    cache.setQueryData(['user','u1'],{...person,description:'Public profile'});
    const me={...person,display_name:'Ada Research',avatar:{text:'AR',color:1},email:'private@example.test'};
    await applyChanges(findAct('PATCH','/me',{answer:me})?.changes ?? [],cache);
    expect(cache.getQueryData(['me'])).toEqual(me);
    expect(cache.getQueryData(['provider','p1'])).toMatchObject({owner:{display_name:'Ada Research',avatar:{text:'AR'}}});
    expect(JSON.stringify(cache.getQueryData(['provider','p1']))).not.toContain('private@example.test');
    expect(cache.getQueryData(['user','u1'])).toEqual({...person,display_name:'Ada Research',avatar:me.avatar,description:'Public profile'});
    cache.clear();
  });

  it('adds a returned token without caching its secret, and removes only that token on revoke', async()=>{
    const cache=new QueryClient();cache.setQueryData(['tokens'],[]);
    const info={id:'token-1',name:'Deploy',scopes:['thread.view'],node:null,created_at:'now',last_used_at:null,expires_at:null};
    await applyChanges(findAct('POST','/me/tokens',{answer:{token:'one-time secret',info,already_created:false}})?.changes ?? [],cache);
    expect(cache.getQueryData(['tokens'])).toEqual([info]);
    expect(JSON.stringify(cache.getQueryData(['tokens']))).not.toContain('one-time secret');
    await applyChanges(findAct('DELETE','/me/tokens/token-1')?.changes ?? [],cache);
    expect(cache.getQueryData(['tokens'])).toEqual([]);cache.clear();
  });

  it('applies a newly opened browser to its native list while preserving capabilities',async()=>{
    const cache=new QueryClient();const key=['environment-browsers','e1'];
    cache.setQueryData(key,{browsers:[],profiles:[{name:'Work'}],off:null,found:['chromium']});
    const browser={id:'browser-1',environment:{id:'e1'},profile:'Work',state:'open'};
    await applyChanges(findAct('POST','/environments/e1/browsers',{answer:browser})?.changes ?? [],cache);
    expect(cache.getQueryData(key)).toEqual({browsers:[browser],profiles:[{name:'Work'}],off:null,found:['chromium']});
    await applyChanges(findAct('DELETE','/browsers/browser-1',{answer:{...browser,state:'closed'}})?.changes ?? [],cache);
    expect(cache.getQueryData(key)).toMatchObject({browsers:[{id:'browser-1',state:'closed'}],profiles:[{name:'Work'}]});cache.clear();
  });

  it('replaces the detail, a chooser, every loaded cursor page and an embedded name, preserving their shapes', async () => {
    const cache = new QueryClient();
    const listKey = ['everything', 'u1', 'resources', { kinds: ['provider'], q: '', resource: { details: true } }];
    cache.setQueryData(['provider', 'p1'], before);
    cache.setQueryData(['providers', 'usable', null], [before]);
    cache.setQueryData(listKey, { pages: [{ items: [{ id: 'other', name: 'Elsewhere', resource: { kind: 'provider', value: { ...before, id: 'other' } } }], next_cursor: 'page-2' }, { items: [{ id: 'p1', name: before.name, resource: { kind: 'provider', value: before } }], next_cursor: null }], pageParams: [null, 'page-2'] });
    cache.setQueryData(['thread', 't1'], { binding: { provider: { id: 'p1', name: before.name } } });
    const reads = vi.spyOn(cache, 'invalidateQueries');
    await applyChanges(findAct('PATCH', '/providers/p1', { answer: { provider: after, job: null } })?.changes ?? [], cache);
    expect(cache.getQueryData(['provider', 'p1'])).toEqual(after);
    expect(cache.getQueryData(['providers', 'usable', null])).toEqual([after]);
    expect(cache.getQueryData(listKey)).toMatchObject({ pages: [{ next_cursor: 'page-2' }, { items: [{ name: after.name, resource: { value: after } }] }], pageParams: [null, 'page-2'] });
    expect(cache.getQueryData(['thread', 't1'])).toEqual({ binding: { provider: { id: 'p1', name: after.name } } });
    expect(reads).not.toHaveBeenCalled();
    cache.clear();
  });

  it('uses the native inventory projection for new policy fields, then replaces/removes matching rows in place', async () => {
    const cache = new QueryClient();
    const key = ['providers', 'u1', 'resources', { kinds: ['provider'], q: 'Research', resource: { details: true } }];
    cache.setQueryData(key, { pages: [{ items: [], next_cursor: null }], pageParams: [null] });
    let current=after;
    const read=vi.fn(()=>Promise.resolve({pages:[{items:[{id:'p1',name:current.name,my_access:'take_part',can_share:true,resource:{kind:'provider',value:current}}],next_cursor:null}],pageParams:[null]}));
    const observer=new QueryObserver(cache,{queryKey:key,queryFn:read,staleTime:Infinity});const off=observer.subscribe(()=>undefined);
    await applyChanges([{row:{kind:'provider',id:'p1',value:after,insert:true}}],cache);
    await applyChanges([{row:{kind:'provider',id:'p1',value:after,insert:true}}],cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toMatchObject({ pages: [{ items: [{ id: 'p1', name: after.name,my_access:'take_part',can_share:true }] }] });
    applyRow(cache, { kind: 'provider', id: 'p1', value: { ...before, revision: 3 } });
    expect(cache.getQueryData(key)).toMatchObject({ pages: [{ items: [] }] });
    current={...after,revision:4};
    await applyChanges([{row:{kind:'provider',id:'p1',value:current,insert:true}}],cache);
    applyRow(cache, { kind: 'provider', id: 'p1', value: null });
    expect(cache.getQueryData(key)).toMatchObject({ pages: [{ items: [] }] });
    off();cache.clear();
  });

  it('replaces a full native DTO so removed config fields and newly supplied fields are accurate',()=>{
    const cache=new QueryClient();cache.setQueryData(['providers'],[{...before,config:{removed:true}}]);
    const row={...after,config:{added:true}};applyRow(cache,{kind:'provider',id:'p1',value:row});
    expect(cache.getQueryData(['providers'])).toEqual([row]);cache.clear();
  });

  it('does not invent rich detail from a minimal private name', () => {
    const cache = new QueryClient();
    cache.setQueryData(['everything'], [{ id: 'p1', name: 'Private', content_hidden: true }]);
    expect(cachedRow(cache, 'provider', 'p1')).toBeUndefined();
    cache.setQueryData(['providers'], [before]);
    expect(cachedRow(cache, 'provider', 'p1')).toEqual(before);
    cache.clear();
  });

  it('keeps a known nested-home row when a rename does not carry its ancestor path',()=>{
    const cache=new QueryClient();const key=['providers','u1','resources',{kinds:['provider'],q:'',resource:{home:'root',details:true}}];
    const value={...before,home:{id:'department',name:'Research'}};
    cache.setQueryData(key,{pages:[{items:[{id:'p1',name:value.name,resource:{kind:'provider',value}}],next_cursor:null}],pageParams:[null]});
    applyRow(cache,{kind:'provider',id:'p1',value:{...value,name:'Renamed provider',revision:2}});
    expect(cache.getQueryData(key)).toMatchObject({pages:[{items:[{id:'p1',name:'Renamed provider'}]}]});
    cache.clear();
  });

  it('updates live references without rewriting captured model inputs or user JSON that happens to contain an id',()=>{
    const cache=new QueryClient();const captured={id:'p1',name:'Captured name'};
    cache.setQueryData(['model-inputs','t1'],{body:{provider:captured},raw:captured,config:{choice:captured},binding:{provider:{id:'p1',name:before.name}}});
    applyRow(cache,{kind:'provider',id:'p1',value:after});
    expect(cache.getQueryData(['model-inputs','t1'])).toEqual({body:{provider:captured},raw:captured,config:{choice:captured},binding:{provider:{id:'p1',name:after.name}}});
    cache.clear();
  });

  it('does not turn a tree summary into a pending detail’s full DTO',()=>{
    const cache=new QueryClient();
    const observer=new QueryObserver(cache,{queryKey:['node','project'],queryFn:()=>Promise.resolve({id:'project',name:'Full project',access:{permissions:['project.view']}}),enabled:false});
    const off=observer.subscribe(()=>undefined);
    applyRow(cache,{kind:'node',id:'project',value:{id:'project',name:'Renamed project',parent_id:'org'},partial:true});
    expect(cache.getQueryData(['node','project'])).toBeUndefined();
    off();cache.clear();
  });

  it('uses the same update path for another person’s live row and does not fetch on reconnect news', async () => {
    const cache = new QueryClient();
    cache.setQueryData(['provider', 'p1'], before);
    cache.setQueryData(['providers'], [before]);
    const reads = vi.spyOn(cache, 'invalidateQueries');
    await applyChanges(resourceChanges('provider', 'p1', { row: after }), cache);
    expect(cache.getQueryData(['provider', 'p1'])).toEqual(after);
    expect(cache.getQueryData(['providers'])).toEqual([after]);
    expect(reads).not.toHaveBeenCalled();
    cache.clear();
  });

  it('carries both JSON values to listeners without consuming the caller’s answer or issuing another request', async () => {
    const fetch = vi.fn(() => Promise.resolve(Response.json({ provider: after, job: null })));
    const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch });
    const listener = vi.fn<(act: Act) => void>();
    client.onAct(listener);
    const body = { name: after.name, expected_revision: 1 };
    const reply = await client.api.PATCH('/providers/{id}', { params: { path: { id: 'p1' } }, body });
    expect(listener).toHaveBeenCalledWith({ readerVersion: client.session.getState().readerVersion ?? 0, method: 'PATCH', path: '/providers/p1', input: body, answer: { provider: after, job: null } });
    expect(reply.data).toEqual({ provider: after, job: null });
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});

describe('one key and no background refresh', () => {
  it('changes session reach without reloading personal preferences or unopened objects',async()=>{
    const cache=new LiveQueryClient();const preferences={sidebar:{folded:['p1']}};
    const readPreferences=vi.fn(()=>Promise.resolve(preferences));const readProvider=vi.fn(()=>Promise.resolve(after));
    cache.setQueryData(['me-state'],preferences);cache.setQueryData(['provider','p1'],before);cache.setQueryData(['provider','private'],before);
    const own=new QueryObserver(cache,{queryKey:['me-state'],queryFn:readPreferences,staleTime:Infinity});
    const shown=new QueryObserver(cache,{queryKey:['provider','p1'],queryFn:readProvider,staleTime:Infinity});
    const offOwn=own.subscribe(()=>undefined);const offShown=shown.subscribe(()=>undefined);
    const reset=vi.spyOn(cache,'resetQueries');
    await applyChanges([{session:true,mode:{as_admin:false,available:true}}],cache);
    expect(cache.getQueryData(['admin-mode'])).toEqual({as_admin:false,available:true});
    expect(readPreferences).not.toHaveBeenCalled();expect(readProvider).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(['provider','private'])).toBeUndefined();expect(reset).not.toHaveBeenCalled();
    offOwn();offShown();cache.clear();
  });

  it('repairs only one visible projection when a genuine empty answer cannot carry its result',async()=>{
    const cache=new LiveQueryClient();const first=vi.fn(()=>Promise.resolve(['new']));const second=vi.fn(()=>Promise.resolve(['unrelated']));
    cache.setQueryData(['channels','one'],[]);cache.setQueryData(['channels','two'],[]);
    const one=new QueryObserver(cache,{queryKey:['channels','one'],queryFn:first,staleTime:Infinity});
    const two=new QueryObserver(cache,{queryKey:['channels','two'],queryFn:second,staleTime:Infinity});
    const offOne=one.subscribe(()=>undefined);const offTwo=two.subscribe(()=>undefined);
    await applyChanges([{missing:[{key:['channels']}]}],cache);
    expect(first).toHaveBeenCalledTimes(1);expect(second).not.toHaveBeenCalled();
    offOne();offTwo();cache.clear();
  });

  it('shares job and setting identities across every reader', () => {
    expect(forkJob('j1').queryKey).toEqual(exportJob('j1').queryKey);
    expect(forkJob('j1').queryKey).toEqual(retentionJobQuery('j1').queryKey);
    expect(targetSettingsKey({ kind: 'provider', id: 'p1' })).toEqual(providerSettingsKey('p1'));
    expect(targetSettingsKey({ kind: 'environment', id: 'e1' })).toEqual(environmentSettingsKey('e1'));
    expect(queryClient.getDefaultOptions().queries?.refetchOnWindowFocus).toBe(false);
    expect(queryClient.getDefaultOptions().queries?.refetchOnReconnect).toBe(false);
  });

  it('does not let a legacy broad callback stale or refetch a list after an answer patched it', async () => {
    const cache = new LiveQueryClient();
    const read = vi.fn(() => Promise.resolve([before]));
    cache.setQueryData(['providers'], [before]);
    const observer = new QueryObserver(cache, { queryKey: ['providers'], queryFn: read, staleTime: Infinity });
    const off = observer.subscribe(() => undefined);
    applyRow(cache, { kind: 'provider', id: 'p1', value: after });
    await cache.invalidateQueries({ queryKey: ['providers'] });
    expect(cache.getQueryData(['providers'])).toEqual([after]);
    expect(cache.getQueryState(['providers'])?.isInvalidated).toBe(false);
    expect(read).not.toHaveBeenCalled();
    off(); cache.clear();
  });
});
