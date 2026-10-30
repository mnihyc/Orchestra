/** Real resource pages share one bounded fixture for their lifecycle confirmations. */
import tokenDensity from '../../../crates/control/engine/data/token-density.json' with {type:'json'};
import { expect, type Page } from '@playwright/test';
import type { Agent, Environment, FactTarget, Provider, ProviderModel } from '../../src/generated/api-types';

export type Kind = 'provider' | 'model' | 'agent' | 'environment';
export interface Fixture {
  target: FactTarget;
  name: string;
  path: string;
  resourcePath: string;
  feed: string;
  mutation: string;
  method: string;
  body: unknown;
  read: () => unknown;
  enable: () => void;
}

export async function resourceFixture(page: Page, kind: Kind, headers: Record<string,string>, available=false): Promise<Fixture> {
  if (kind === 'provider' || kind === 'model') {
    const listed = await page.request.get('/api/v1/providers',{headers}); expect(listed.ok()).toBe(true);
    const base = (await listed.json() as {items:Provider[]}).items.find((row)=>row.access.permissions.includes('provider.manage'));
    if (base === undefined) throw new Error('The disposable person needs a provider they manage');
    const full = await page.request.get(`/api/v1/providers/${base.id}`,{headers}); expect(full.ok()).toBe(true);
    let row = await full.json() as Provider;
    const model:ProviderModel = {id:'019a0000-0000-7000-8000-000000000041',model_name:'fixture-model',display_name:'Review model',enabled:available,source:'manual',groups:[],parameters:{},token_estimate:{...tokenDensity.unmeasured,measured:false}};
    row = {...row,status:kind==='provider'&&!available?'disabled':'ready',...(kind==='model'?{models:[model]}:{})};
    return {target:{object:{kind:'provider',id:row.id},model_id:kind==='model'?model.id:null},name:kind==='model'?model.display_name:row.name,
      path:`/resources/providers/${row.id}${kind==='model'?'?tab=models':''}`,resourcePath:`/api/v1/providers/${row.id}`,feed:`resource:provider:${row.id}`,
      mutation:kind==='model'?`/api/v1/providers/${row.id}/models/${model.id}`:`/api/v1/providers/${row.id}/enable`,method:kind==='model'?'PATCH':'POST',body:kind==='model'?{enabled:true}:null,
      read:()=>row,enable:()=>{row=kind==='model'?{...row,models:row.models.map((entry)=>({...entry,enabled:true}))}:{...row,status:'ready'};}};
  }
  if (kind === 'agent') {
    const listed=await page.request.get('/api/v1/agents',{headers});expect(listed.ok()).toBe(true);
    const base=(await listed.json() as {items:Agent[]}).items.find((row)=>row.access.permissions.includes('agent.manage'));
    if(base===undefined)throw new Error('The disposable person needs a profile they manage');
    const full=await page.request.get(`/api/v1/agents/${base.id}`,{headers});expect(full.ok()).toBe(true);
    let row={...await full.json() as Agent,disabled:!available};
    return {target:{object:{kind:'agent',id:row.id},model_id:null},name:row.name,path:`/resources/agents/${row.id}`,resourcePath:`/api/v1/agents/${row.id}`,feed:`resource:agent:${row.id}`,
      mutation:`/api/v1/agents/${row.id}`,method:'PATCH',body:{expected_revision:row.revision,disabled:false},read:()=>row,enable:()=>{row={...row,disabled:false,revision:row.revision+1};}};
  }
  const listed=await page.request.get('/api/v1/environments',{headers});expect(listed.ok()).toBe(true);
  const base=(await listed.json() as {items:Environment[]}).items.find((row)=>row.access.permissions.includes('environment.manage'));
  if(base===undefined)throw new Error('The disposable person needs an environment they manage');
  const full=await page.request.get(`/api/v1/environments/${base.id}`,{headers});expect(full.ok()).toBe(true);
  let row:Environment={...await full.json() as Environment,status:available?'stopped':'disabled'};
  return {target:{object:{kind:'environment',id:row.id},model_id:null},name:row.name,path:`/resources/environments/${row.id}`,resourcePath:`/api/v1/environments/${row.id}`,feed:`resource:environment:${row.id}`,
    mutation:`/api/v1/environments/${row.id}/enable`,method:'POST',body:null,read:()=>row,enable:()=>{row={...row,status:'stopped'};}};
}

export async function resourceLive(page:Page,source:Fixture) {
  // The authorized native row also reaches the page through cached directory
  // entries and the user feed. All fixture projections must agree with its point read.
  const mirror=(value:unknown):unknown=>{
    if(Array.isArray(value))return value.map(mirror);
    if(typeof value!=='object'||value===null)return value;
    const row=value as Record<string,unknown>;
    if(row.id===source.target.object.id&&typeof row.revision==='number'&&'access'in row&&row.content_hidden!==true
      &&(source.target.object.kind==='agent'?typeof row.disabled==='boolean':typeof row.status==='string')){
      const controlled=source.read();
      if(typeof controlled!=='object'||controlled===null||Array.isArray(controlled))return value;
      const fields=controlled as Record<string,unknown>;
      // Lifecycle state is controlled; current native authority and metadata remain intact.
      return source.target.object.kind==='agent'?{...row,disabled:fields.disabled,revision:fields.revision}
        :{...row,status:fields.status,revision:fields.revision,...(typeof source.target.model_id==='string'?{models:fields.models}:{})};
    }
    return Object.fromEntries(Object.entries(row).map(([key,next])=>[key,mirror(next)]));
  };
  const list=source.resourcePath.slice(0,source.resourcePath.lastIndexOf('/'));
  for(const route of [/\/api\/v1\/orgs\/[^/]+\/tree(?:\?|$)/u,new RegExp(`${list}(?:\\?|$)`,'u')]){
    await page.route(route,async(intercept)=>{
      const response=await intercept.fetch();
      if(!response.ok()){await intercept.fulfill({response});return;}
      await intercept.fulfill({response,json:mirror(await response.json())});
    });
  }
  const listeners=new Set<()=>void>();
  await page.route(new RegExp(`${source.resourcePath}(?:\\?|$)`,'u'),(route)=>route.fulfill({status:200,json:source.read()}));
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/u,(ws)=>{
    const server=ws.connectToServer();const ids=new Set<number>();let closed=false;
    server.onMessage((raw)=>{
      let value:unknown;
      try{value=JSON.parse(String(raw));}catch{ws.send(raw);return;}
      ws.send(JSON.stringify(mirror(value)));
    });
    const send=()=>{if(!closed)for(const id of ids)ws.send(JSON.stringify({op:'event',id,feed:source.feed,data:{row:source.read()}}));};listeners.add(send);
    ws.onMessage((raw)=>{
      const frame=JSON.parse(String(raw)) as {op:string;id?:number;feed?:string};
      if(frame.op==='sub'&&frame.id!==undefined&&frame.feed===source.feed){ids.add(frame.id);ws.send(JSON.stringify({op:'subscribed',id:frame.id,feed:source.feed,seq:null,mode:'ephemeral'}));send();}
      else if(frame.op==='unsub'&&frame.id!==undefined&&ids.has(frame.id)){ids.delete(frame.id);ws.send(JSON.stringify({op:'closed',id:frame.id,feed:source.feed,reason:'unsubscribed'}));}
      else server.send(raw);
    });
    ws.onClose((code,reason)=>{closed=true;listeners.delete(send);void server.close({...(code===undefined?{}:{code}),...(reason===undefined?{}:{reason})});});
  });
  return ()=>{for(const send of listeners)send();};
}

