/** Bilingual click path on the production provider screen; view fixtures are separate from native sealing tests. */
import { expect,test } from '@playwright/test';
import type { Provider } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with {type:'json'};
import chinese from '../../src/i18n/zh-CN.json' with {type:'json'};
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn, arrived } from './support';
interface Write{expected_revision:number;facts?:{by:string;secrets?:Record<string,string|null>};models?:unknown;presenter?:unknown}
const source={name:'openai',title:'OpenAI',summary:'',contract:1,facts:{reports:['spend'],field_reports:{},spends_resets:false,credential:'none',min_interval_secs:null,plans:{},admin_credential:{labels:{en:'Admin key','zh-CN':'管理密钥'},reports:['spend']}},models:null,presents:null,fields:[{name:'admin_key',label:'Admin key',type:'secret',required:false,help:null,choices:null,default:null,max_bytes:null}],docs_url:null};
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)test(`provider admin credential is separate and write-only ${locale}/${theme}`,async({page,request},info)=>{
  await scenario();await page.emulateMedia({colorScheme:theme});
  const double=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
  expect((await request.post(`${double}/__double/first-steps/appearance`,{data:{locale,theme:'system'}})).ok()).toBe(true);
  await signIn(page);
  const id=RUNS_ON.openAiKey,path=`/api/v1/providers/${id}`;
  const sourceView=`${double}/__double/resources/provider-source-view`;
  const read=await request.post(sourceView,{data:{id}});expect(read.ok()).toBe(true);
  const original=await read.json() as Provider;let saved=false;let revision=0;const writes:Write[]=[];
  const shown=()=>{
    return {...original,revision:original.revision+revision,sources:{...original.sources,facts:{...original.sources.facts,by:'default',name:'openai',descriptor:source,config:{},secrets:saved?['admin_key']:[]}},secrets_set:[...original.secrets_set.filter((name)=>name!=='source:facts:field:admin_key'),...(saved?['source:facts:field:admin_key']:[])],facts:{...original.facts,balances:[],spend:[]},facts_state:{...original.facts_state,balances:'unsupported',spend:saved?'not_read_yet':{unsupported_because:{reason:'needs_admin_key'}}}};
  };
  const publish=async()=>{
    const response=await request.post(sourceView,{data:{id,view:shown()}});expect(response.ok()).toBe(true);
    const provider=await response.json() as Provider;revision=provider.revision-original.revision;return provider;
  };
  await publish();
  await page.route(`**${path}/sources`,async(route)=>{
    if(route.request().method()!=='PUT'){await route.continue();return;}
    const body=route.request().postDataJSON() as Write;writes.push(body);
    expect(body.expected_revision).toBe(original.revision+revision);
    expect(body.facts?.by).toBe('default');expect(body.models).toBeUndefined();expect(body.presenter).toBeUndefined();
    const key=body.facts?.secrets?.admin_key;if(key!==undefined)saved=key!==null;
    await route.fulfill({json:await publish()});
  });
  const words=locale==='en'?english:chinese,label=locale==='en'?'Admin key':'管理密钥';
  await page.goto(`/resources/providers/${id}?tab=settings`);
  await expect(page.locator('html')).toHaveAttribute('lang',locale);
  const group=page.getByRole('group',{name:words.providerSources.detailsFrom,exact:true});
  const field=group.getByLabel(`${label} ${words.ui.field.optional}`,{exact:true});
  await expect(field).toHaveValue('');await expect(field).toHaveAttribute('type','password');
  await field.fill('fixture-admin-only');
  const savedReply=arrived(page.waitForResponse((reply)=>reply.request().method()==='PUT'&&new URL(reply.url()).pathname===path+'/sources'));
  await page.getByRole('button',{name:words.providerSources.save,exact:true}).click();
  expect(await (await savedReply).text()).not.toContain('fixture-admin-only');
  await expect(field).toHaveValue('');expect(writes[0]?.facts?.secrets).toEqual({admin_key:'fixture-admin-only'});
  for(const width of [1440,400]){
    await page.setViewportSize({width,height:900});await expect(field).toBeVisible();
    const box=await field.boundingBox();expect(box).not.toBeNull();if(box!==null){expect(box.x).toBeGreaterThanOrEqual(0);expect(box.x+box.width).toBeLessThanOrEqual(width);}
    await info.attach(`source-admin-${locale}-${theme}-${width}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
  }
  await group.getByRole('button',{name:words.providerSources.removeSavedKey,exact:true}).click();
  await expect(group.getByText(words.providerSources.keyWillBeRemoved,{exact:true})).toBeVisible();
  const removedReply=page.waitForResponse((reply)=>reply.request().method()==='PUT'&&new URL(reply.url()).pathname===path+'/sources');
  await page.getByRole('button',{name:words.providerSources.save,exact:true}).click();await removedReply;
  expect(writes[1]?.facts?.secrets).toEqual({admin_key:null});
  await page.goto(`/resources/providers/${id}?tab=overview`);
  // Unsupported members share one current source summary; retain the specific affected member.
  await expect(page.locator('[data-needs-admin-members]')).toHaveText(words.providerDetails.needsAdminKeyMembers.replace('{{members}}',words.providerDetails.members.spend));
  expect(await page.locator('[data-fact-member="balances"]').count()).toBe(0);
});
