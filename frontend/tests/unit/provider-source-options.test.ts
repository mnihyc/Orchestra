/** Defaults follow served host metadata and a source write keeps configuration and secret values apart. */
import { expect, it } from 'vitest';
import { emptySourceDraft } from '@/features/resources/providers/SourceFields';
import { defaultOffer, sourceChoiceSet } from '@/features/resources/providers/sourceOptions';
import type { Field, ProviderSetupSource } from '@/generated/api-types';
import { SOURCE_DESCRIPTOR } from './provider-plan-fixtures';

it('selects a default from the declared exact endpoint host, independently of a known vendor name', () => {
  const offer:ProviderSetupSource={by:'builtin',name:'a-server-declared-reader',descriptor:SOURCE_DESCRIPTOR,default_for:['plans.example'],machine:null,template:null,command:null};
  expect(defaultOffer([offer],'facts',[{base_url:'https://PLANS.example/v1'}],false)?.name).toBe(offer.name);
  expect(defaultOffer([offer],'facts',[{base_url:'https://plans.example.other/v1'}],false)).toBeUndefined();
});

it('writes public booleans and maps as config, secret maps as secret text, and neither declared secret defaults nor stale hidden fields', () => {
  const field=(name:string,type:Field['type'],value:unknown=null):Field => ({name,type,label:name,required:false,default:value,help:null,choices:null,max_bytes:null});
  const fields=[field('enabled','boolean'),field('headers','map'),field('token','secret','do-not-send'),field('passwords','secret_map')];
  const draft={...emptySourceDraft(),by:'program' as const,program:'template:local-name',fields:{enabled:false,headers:{Accept:'application/json'},passwords:{a:'new-secret'},hidden:'stale-secret'}};
  const body=sourceChoiceSet(draft,fields,[{name:'local-name',choice:'template:local-name',title:'Reports',summary:'',reports:[],fields,command:null,templateId:'template'}]);
  expect(body).toEqual({by:'program',name:'local-name',config:{enabled:false,headers:{Accept:'application/json'}},secrets:{passwords:'{"a":"new-secret"}'}});
  expect(JSON.stringify(body)).not.toContain('do-not-send');
  expect(JSON.stringify(body)).not.toContain('stale-secret');
  expect(sourceChoiceSet(emptySourceDraft(),fields,[])).toEqual({by:'default'});
});
