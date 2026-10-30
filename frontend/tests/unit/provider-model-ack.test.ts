/** Native model and provider acknowledgements update only the projections and standing they carry. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it } from 'vitest';
import { answerChanges } from '@/api/actAnswers';
import { applyChanges } from '@/api/acts';
import { applyCollection } from '@/api/cache';
import { providerKey } from '@/api/resources';
import type { Act } from '@/api/client';
import type { Provider, ProviderModel } from '@/generated/api-types';
import { planProvider } from './provider-plan-fixtures';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const cache=new QueryClient();
afterEach(()=>cache.clear());
it('keeps a model write in both collections while preserving parent fields the reply does not carry',()=>{
  const provider=planProvider({id:'p1',models:[],status:'needs_setup'});
  const other=planProvider({id:'p2',models:[]});
  cache.setQueryData(providerKey('p1'),provider);cache.setQueryData(providerKey('p2'),other);
  cache.setQueryData(['provider-models','p1'],[]);
  const model:ProviderModel={ ...MODEL_ESTIMATE,id:'large',model_name:'Large example model',display_name:'Large example model',enabled:true,source:'manual',groups:[],parameters:{}};
  const action:Act={method:'POST',path:'/providers/{id}/models',input:{model_name:model.model_name},answer:model};
  const changes=answerChanges(action,(name)=>name==='id'?'p1':'',[{key:['provider'],id:'p1'}]);
  expect(changes).toHaveLength(2);
  for(const change of changes){expect(change).toHaveProperty('collection');if('collection' in change)applyCollection(cache,change.collection);}
  const current=cache.getQueryData<Provider>(providerKey('p1'));
  expect(current?.models).toEqual([model]);
  expect(cache.getQueryData(['provider-models','p1'])).toEqual([model]);
  expect(current?.status).toBe('needs_setup');expect(current?.access).toBe(provider.access);
  expect(cache.getQueryData(providerKey('p2'))).toBe(other);
});

it('uses the acceptance reply for the complete provider and both model collections, including native readiness',async()=>{
  const before=planProvider({id:'p1',models:[],status:'needs_setup',status_detail:'it needs an enabled model'});
  const other=planProvider({id:'p2',models:[]});
  const model:ProviderModel={ ...MODEL_ESTIMATE,id:'gpu-small',model_name:'gpu-small',display_name:'GPU Small',enabled:true,source:'suggested',groups:[],parameters:{}};
  const accepted={...before,status:'ready' as const,status_detail:null,models:[model]};
  cache.setQueryData(providerKey('p1'),before);cache.setQueryData(providerKey('p2'),other);cache.setQueryData(['provider-models','p1'],[]);
  const action:Act={method:'POST',path:'/providers/{id}/models/suggestions/accept',input:{model_names:['gpu-small']},answer:{added:[model],provider:accepted}};
  const changes=answerChanges(action,(name)=>name==='id'?'p1':'',[{key:['provider'],id:'p1'}]);
  await applyChanges(changes,cache);
  expect(cache.getQueryData(providerKey('p1'))).toEqual(accepted);expect(cache.getQueryData(['provider-models','p1'])).toEqual([model]);
  expect(cache.getQueryData(providerKey('p2'))).toBe(other);expect(cache.getQueryState(providerKey('p1'))?.isInvalidated).toBe(false);
});
