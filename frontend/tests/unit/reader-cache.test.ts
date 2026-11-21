/** A direct change of person retires the same private cache as sign-out. */
import { QueryClient } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { metaKey } from '@/api/auth';
import { client, createApiClient } from '@/api/client';
import { poolMembersQuery, poolQuery, readPoolAfterWrite } from '@/api/pools';
import { applyChanges, findAct } from '@/api/acts';
import { libraryQuery } from '@/api/resourceLibrary';
import type { PoolMembersReply, LibraryEntry } from '@/generated/api-types';
import { installReaderCacheBoundary } from '@/api/readerCache';
import type { AuthResult } from '@/generated/api-types';
import { me } from '../doubles/fixtures';

const reads = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }));
vi.mock('@/api/client', async (original) => ({ ...await original<object>(), api: {GET:reads.get, POST:reads.post} }));

const grant = (id: number, username: string): AuthResult => ({
  user: me(id, username, username), access_token: `test-${username}`, access_expires_at: new Date(Date.now() + 900_000).toISOString(),
});

it('keeps same-person renewals but clears private data on a direct account change and sign-out', async () => {
  const cache = new QueryClient();
  const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: () => Promise.resolve(new Response(null, { status: 204 })) });
  const stop = installReaderCacheBoundary(client, cache);
  try {
    client.adopt(grant(1, 'first'));
    cache.setQueryData(metaKey, { may_create_org: true, name: 'Public' });
    cache.setQueryData(['reference-preview', 'document', 'private'], 'Private detail');
    client.adopt(grant(1, 'first'));
    expect(cache.getQueryData(['reference-preview', 'document', 'private'])).toBe('Private detail');
    client.adopt(grant(2, 'second'));
    expect(cache.getQueryData(['reference-preview', 'document', 'private'])).toBeUndefined();
    expect(cache.getQueryData(metaKey)).toMatchObject({ name: 'Public', may_create_org: false });
    cache.setQueryData(['private-second'], 'Second person');
    await client.signOut();
    expect(cache.getQueryData(['private-second'])).toBeUndefined();
    expect(cache.getQueryData(metaKey)).toMatchObject({ name: 'Public', may_create_org: false });
  } finally { stop(); cache.clear(); }
});

it('does not restore a previous reader’s result when a request finishes late', async () => {
  const cache = new QueryClient();
  const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: () => Promise.resolve(new Response(null, { status: 204 })) });
  const stop = installReaderCacheBoundary(client, cache);
  try {
    client.adopt(grant(1, 'first'));
    let finish: (value: string) => void = () => { throw new Error('Read was not started'); };
    const pending = cache.query({ queryKey: ['private'], queryFn: () => new Promise<string>((resolve) => { finish = resolve; }) });
    client.adopt(grant(2, 'second'));
    finish('Late private data');
    await pending.catch(() => undefined);
    expect(cache.getQueryData(['private'])).toBeUndefined();
    await client.signOut();
  } finally { stop(); cache.clear(); }
});


it('reads a pool member again after the same person changes reader context', async () => {
  const before=client.session.getState();
  const cache=new QueryClient();
  const row:PoolMembersReply['items'][number]={id:'member',position:0,enabled:true,priority:null,weight:null,
    rule:{type:'single',resource:{kind:'host',id:'private-host'}},label:'Private host',reason:null};
  try {
    client.session.setState({userId:'pool-reader',readerVersion:1,status:'signed_in'});
    reads.get.mockResolvedValueOnce({data:{items:[row],next:null},response:new Response(null,{status:200})})
      .mockResolvedValueOnce({data:{items:[{...row,rule:null,label:null,reason:'member_hidden'}],next:null},response:new Response(null,{status:200})});
    const first=await cache.query({...poolMembersQuery('pool'),staleTime:Infinity});
    expect(first.items[0]?.label).toBe('Private host');
    client.session.setState({readerVersion:2});
    const second=await cache.query({...poolMembersQuery('pool'),staleTime:Infinity});
    expect(second.items[0]?.label).toBeNull();
    expect(second.items[0]?.rule).toBeNull();
    expect(reads.get).toHaveBeenCalledTimes(2);
  } finally {cache.clear();reads.get.mockReset();client.session.setState(before);}
});

it('reads a pool reference figure again after the same person changes reader context', async () => {
  const before=client.session.getState();
  const cache=new QueryClient();
  const target={object:{kind:'pool' as const,id:'pool'},version_id:null};
  const entry:LibraryEntry={target,threads:null,facts:{subject:{kind:'resource',object:target.object},as_of:'2026-11-01T00:00:00Z',dependents_complete:true,uses:null,
    dependents:[{relation:'explicit_giving',count:1,hidden:0,until:null,visible:[{object:null,name:'Private settings place',relation:'explicit_giving',until:null}]}]}};
  try {
    client.session.setState({userId:'pool-reader',readerVersion:1,status:'signed_in'});
    reads.post.mockResolvedValueOnce({data:[entry],response:new Response(null,{status:200})})
      .mockResolvedValueOnce({data:[{...entry,facts:{...entry.facts,dependents:[{...entry.facts.dependents[0],visible:[],hidden:1}]}}],response:new Response(null,{status:200})});
    const first=await cache.query({...libraryQuery([target]),staleTime:Infinity});
    expect(first[0]?.facts.dependents[0]?.visible[0]?.name).toBe('Private settings place');
    client.session.setState({readerVersion:2});
    const second=await cache.query({...libraryQuery([target]),staleTime:Infinity});
    expect(second[0]?.facts.dependents[0]?.visible).toEqual([]);
    expect(reads.post).toHaveBeenCalledTimes(2);
  } finally {cache.clear();reads.post.mockReset();client.session.setState(before);}
});

it('refuses a late pool acknowledgement read after its permission cut even when the person is unchanged',async()=>{
  const before=client.session.getState(),cache=new QueryClient();
  let finish:((value:unknown)=>void)|undefined;
  reads.get.mockReturnValueOnce(new Promise((resolve)=>{finish=resolve;}));
  client.session.setState({userId:'pool-reader',readerVersion:77,status:'signed_in'});
  try {
    const acknowledgement=readPoolAfterWrite(cache,'pool').catch((error:unknown)=>error);
    await vi.waitFor(()=>expect(reads.get).toHaveBeenCalledTimes(1));
    await applyChanges(findAct('PATCH','/pools/pool/permissions',{answer:{}})?.changes??[],cache);
    finish?.({data:{id:'pool',name:'Former private pool'},response:new Response(null,{status:200})});
    expect(await acknowledgement).toMatchObject({name:'AbortError'});
    expect(cache.getQueryData(poolQuery('pool').queryKey)).toBeUndefined();
  } finally {finish?.({});cache.clear();reads.get.mockReset();client.session.setState(before);}
});
