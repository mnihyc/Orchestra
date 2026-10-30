/** QS-009/QS-029: provider origins, setup reasons and credential requirements stay factual in both catalogues. */
import { beforeAll, describe, expect, it } from 'vitest';
import { i18n, loadSearchCatalogues } from '@/i18n';
import { knownPlaces } from '@/lib/placeLabel';
import { endpointsNeedNoKey } from '@/features/resources/providers/details';
import { providerLabels,providerPickerLabels } from '@/features/resources/providers/labels';
import { PROVIDER_NEXT, providerState, providerWhat } from '@/features/resources/providers/state';
import { PLAN_NOW, planProvider } from './provider-plan-fixtures';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const roots = knownPlaces([
  {id:'acme',name:'Acme',above:[]},
  {id:'beta',name:'Beta',above:[]},
]);
const keyEndpoint = {protocol:'openai-responses',base_url:'https://example.invalid/v1',options:{}};
beforeAll(async () => { await loadSearchCatalogues(); });

describe('QS-009 provider names', () => {
  it('uses the same place labels for equal names across orgs and leaves unique names alone', () => {
    const acme=planProvider({id:'first',name:'Scripted model',org_id:'acme',home:{id:'acme',kind:'org',name:'Acme',redacted:false}});
    const beta=planProvider({id:'second',name:'Scripted model',org_id:'beta',home:{id:'beta',kind:'org',name:'Beta',redacted:false}});
    const unique=planProvider({id:'third',name:'Other model'});
    const labels=providerLabels([acme,beta,unique],roots);
    expect(labels.get(acme.id)).toBe('Scripted model (Acme)');
    expect(labels.get(beta.id)).toBe('Scripted model (Beta)');
    expect(labels.get(unique.id)).toBe('Other model');
  });
  it('does not reveal a cached name when the current origin is redacted', () => {
    const hidden=planProvider({id:'hidden',name:'Same',org_id:'private',home:{id:'private',kind:'org',name:null,redacted:true}});
    const visible=planProvider({id:'visible',name:'Same',org_id:'beta',home:{id:'beta',kind:'org',name:'Beta',redacted:false}});
    const cached=knownPlaces([{id:'private',name:'Private org',above:[]},{id:'beta',name:'Beta',above:[]}]);
    const label=providerLabels([hidden,visible],cached).get(hidden.id);
    expect(label).toBe('Same (Ada Lovelace)');
    expect(label).not.toContain('Private org');
  });
  it('uses readable homes, then owners, and never a compact routing address in a picker',()=>{
    const acme=planProvider({id:'a',name:'Shared',home:{id:'acme',kind:'org',name:'Acme',redacted:false},org_id:'acme'});
    const beta=planProvider({id:'b',name:'Shared',home:{id:'beta',kind:'org',name:'Beta',redacted:false},org_id:'beta'});
    const labels=providerPickerLabels([acme,beta],[acme,beta]);
    expect(labels.get('a')).toBe('Shared (Acme)');expect(labels.get('b')).toBe('Shared (Beta)');
    const colleague=planProvider({...acme,id:'c',owner:{...acme.owner,id:'grace',display_name:'Grace'}});
    const owners=providerPickerLabels([acme,colleague],[acme,colleague]);
    expect(owners.get('a')).toBe('Shared (Ada Lovelace)');expect(owners.get('c')).toBe('Shared (Grace)');
    const compact=providerPickerLabels([{...acme,address:'acme.ada.shared'},{...beta,address:'beta.grace.shared'}]);
    expect(compact.get('a')).toBe('Shared');expect(compact.get('b')).toBe('Shared');
    expect([...compact.values()].join(' ')).not.toMatch(/acme\.ada|beta\.grace/);
  });
});

for (const language of ['en','zh-CN'] as const) describe('QS-029 setup in ' + language, () => {
  const t=i18n.getFixedT(language);
  const state=(provider:ReturnType<typeof planProvider>)=>providerState(provider,t,{locale:language,timeZone:'UTC'},PLAN_NOW);
  it('names the missing key and gives the key act', () => {
    const result=state(planProvider({kind:'api',status:'needs_setup',upstreams:[keyEndpoint]}));
    expect(result).toMatchObject({text:t('providers.phrase.needsKey'),detail:t('providers.overview.needsKeyLead'),next:'key'});
    expect(PROVIDER_NEXT[result.next ?? 'settings'].tab).toBe('key');
  });
  it('does not ask for a key that every endpoint explicitly says it needs none of', () => {
    const provider=planProvider({kind:'api',status:'needs_setup',upstreams:[{...keyEndpoint,options:{auth:'none'}}]});
    expect(endpointsNeedNoKey(provider)).toBe(true);
    expect(state(provider)).toMatchObject({text:t('providers.phrase.needsModel'),next:'models'});
    provider.facts_state.plan='unsupported';
    expect(providerWhat(provider,(kind)=>kind,t)).toBe(t('providers.what.apiOpen',{count:1}));
    provider.upstreams.push({...keyEndpoint,protocol:'anthropic-messages'});
    expect(endpointsNeedNoKey(provider)).toBe(false);
    expect(state(provider).next).toBe('key');
  });
  it('asks a signed-in subscription to enable a model rather than sign in again', () => {
    const provider=planProvider({status:'needs_setup'});
    expect(state(provider).next).toBe('models');
    provider.facts.credential=null;
    expect(state(provider).next).toBe('signIn');
  });
  it('keeps an unexplained setup state actionable and shows its actual explanation when supplied', () => {
    const provider=planProvider({kind:'api',status:'needs_setup',placement:'connected',key_set:true,upstreams:[keyEndpoint],
      models:[{ ...MODEL_ESTIMATE,id:'model',model_name:'example',display_name:'Example',parameters:{},groups:[],enabled:true,source:'manual'}]});
    expect(state(provider)).toMatchObject({next:'settings',detail:t('providers.overview.checkSetupLead')});
    provider.status_detail='The keeping machine needs its endpoint configuration reviewed.';
    expect(state(provider).detail).toBe(provider.status_detail);
  });
  it('points to the runtime when its key and models are ready but no managed runtime exists', () => {
    const provider=planProvider({kind:'api',status:'needs_setup',key_set:true,upstreams:[keyEndpoint],
      models:[{ ...MODEL_ESTIMATE,id:'model',model_name:'example',display_name:'Example',parameters:{},groups:[],enabled:true,source:'manual'}]});
    expect(state(provider)).toMatchObject({text:t('providers.phrase.needsRuntime'),detail:t('providers.overview.needsRuntimeLead'),next:'runtime'});
  });
});
