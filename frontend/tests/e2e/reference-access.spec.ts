/** Private references remain real chips at phone and desktop widths, with one preview read and no hidden metadata. */
import {expect,test,type BrowserContext} from '@playwright/test';
import {PEOPLE,scenario,signIn,dropConnections} from './support.ts';
import {actAs,block,DOCS,held} from './documents-support.ts';
import {id,me,userRef} from '../doubles/fixtures.ts';
import type {Approval,ReferencePreview} from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with {type:'json'};
import chinese from '../../src/i18n/zh-CN.json' with {type:'json'};
const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT??'41871'}`;
for(const width of [400,1440])for(const theme of ['light','dark'] as const)for(const locale of ['en','zh-CN'] as const)test(`private reference card fits ${width}px ${locale}/${theme}`,async({page,request})=>{
 await scenario();await page.setViewportSize({width,height:1000});
 const label=width===400&&theme==='dark'&&locale==='en'?'Shared_rollout_plan_'.repeat(10):'The title as shared';
 const privateTarget=await held(DOCS.private);const source=await held(DOCS.plan);
 await actAs('ada',DOCS.plan,[{op:'update',id:block(1),base_seq:source.seq,content:[{type:'text',text:'Shared work: '},{type:'ref',kind:'document',id:DOCS.private,label,name:null}]}]);
 await signIn(page,PEOPLE.vera);
 const login=await request.post(`${DOUBLE}/api/v1/auth/login`,{data:{login:PEOPLE.vera.login,password:PEOPLE.vera.password}});expect(login.ok()).toBe(true);
 const {access_token:token}=await login.json() as {access_token:string};
 expect((await request.patch(`${DOUBLE}/api/v1/me`,{headers:{Authorization:`Bearer ${token}`},data:{locale,theme}})).ok()).toBe(true);
 let reads=0;page.on('request',one=>{const url=new URL(one.url());if(url.pathname.endsWith('/references/preview')&&url.searchParams.get('id')===DOCS.private)reads+=1;});
 const words=locale==='en'?english:chinese;
 await page.goto(`/doc/${DOCS.plan}`);await expect(page.getByRole('textbox',{name:words.documents.page.body,exact:true})).toBeVisible();
 const lock=page.getByRole('button',{name:label,exact:true});
 await expect(lock).toHaveAttribute('data-reference-state','no-access');
 expect(await lock.evaluate(element=>element.scrollWidth<=element.clientWidth+1)).toBe(true);
  const chipBounds=await lock.boundingBox();expect(chipBounds).not.toBeNull();if(chipBounds===null)throw new Error('No chip bounds');
  expect(chipBounds.x).toBeGreaterThanOrEqual(0);expect(chipBounds.x+chipBounds.width).toBeLessThanOrEqual(width);
 await lock.focus();await lock.press('Enter');
 const card=page.locator('[data-reference-card]').filter({hasText:words.people.reference.noAccess});
 await expect(card).toBeVisible();await expect(card).toContainText(words.people.reference.ownersAsked);
 await expect(card).toContainText(locale==='en'?'Asks for View document':'申请查看文档');
 await expect(card).not.toContainText(privateTarget.title);await expect(card).not.toContainText('Ada Lovelace');
 await expect(card.getByRole('button',{name:words.access.request.send,exact:true})).toBeEnabled();
 const bounds=await card.boundingBox();expect(bounds).not.toBeNull();if(bounds===null)throw new Error('No card bounds');
 expect(bounds.x).toBeGreaterThanOrEqual(0);expect(bounds.x+bounds.width).toBeLessThanOrEqual(width);expect(bounds.y+bounds.height).toBeLessThanOrEqual(1000);
 expect(reads).toBe(1);
 const path=test.info().outputPath(`reference-${width}-${locale}-${theme}.png`);await page.screenshot({path,animations:'disabled'});await test.info().attach('Private reference',{path,contentType:'image/png'});
 await page.keyboard.press('Escape');await expect(card).toHaveCount(0);await expect(lock).toBeFocused();
});

const DAY_MS=24*60*60*1000;

// The transport fixture supplies canonical server outcomes. The browser must send the actual form,
// render the same quick grant at the reference, repair on reconnect and recheck its known deadline.
for(const locale of ['en','zh-CN'] as const)test(`a reference request becomes a one-day grant and greys at expiry (${locale})`,async({browser,context,page,request})=>{
 await scenario();await page.setViewportSize({width:locale==='en'?1440:400,height:1000});
 const words=locale==='en'?english:chinese;
 const source=await held(DOCS.plan),privateTarget=await held(DOCS.private);
 const shared='The title as shared';
 await actAs('ada',DOCS.plan,[{op:'update',id:block(1),base_seq:source.seq,content:[{type:'ref',kind:'document',id:DOCS.private,label:shared,display:'small',name:null}]}]);
 const ada=userRef(me(1,'ada','Ada Lovelace')),vera=userRef(me(8,'vera','Vera Rubin'));
 const approvalId=id(0x71,1),batchId=id(0x71,2);
 const now=Date.now(),end=new Date(now+DAY_MS).toISOString();
 let created=false,given=false,expired=false;
 let sent:unknown=null,decision:unknown=null;
 const approval=(owner:boolean):Extract<Approval,{kind:'access'}>=>({
  id:approvalId,kind:'access',state:given?'approved':'pending',title:shared,node:null,
  object:{kind:'document',id:DOCS.private,name:owner?privateTarget.title:shared},requester:{user:vera},
  request:{permissions:['doc.view'],options:null,holder:{kind:'user',user:vera},reason:'Working together',for_thread:null,label:shared},
  required_permission:'access.manage',can_decide:owner,approver_scope:null,batch_id:batchId,thread_id:null,item_id:null,
  created_at:new Date(now).toISOString(),expires_at:null,quick_grant_durations:['1d','7d','30d','always'],
  decision:given?{approve:true,quick_duration:'1d',expires_at:end}:null,
  decided_at:given?new Date(now).toISOString():null,decided_by:given?ada:null,decided_automatically:false,
 });
 const install=async(client:BrowserContext,owner:boolean)=>{
  await client.route('**/api/v1/references/preview?**',async route=>{
   if(new URL(route.request().url()).searchParams.get('id')!==DOCS.private){await route.continue();return;}
   const base:ReferencePreview={kind:'document',id:DOCS.private,node_kind:null,title:'',description:'',place:null,updated_at:null,updated_by:null,part:null,file:null};
   const body=owner||given&&!expired?{...base,title:privateTarget.title,description:'Current readable description',refresh_at:owner?null:end,access_requests:owner&&created?[approval(true)]:[]}:
    {...base,unavailable:{deleted:false,object:{kind:'document',id:DOCS.private},permission:'doc.view',requestable:true,pending:created&&!given?approvalId:null}};
   await route.fulfill({json:body});
  });
  await client.route('**/api/v1/access-requests',async route=>{
   if(route.request().method()!=='POST'){await route.continue();return;}
   sent=route.request().postDataJSON();created=true;
   await route.fulfill({status:201,json:{batch_id:batchId,approvals:[approval(false)]}});
  });
  await client.route(`**/api/v1/access-requests/${batchId}`,route=>route.fulfill({json:{batch_id:batchId,approvals:[approval(owner)],complete:given}}));
  await client.route(`**/api/v1/approvals/${approvalId}`,route=>route.fulfill({json:approval(owner)}));
  await client.route(`**/api/v1/approvals/${approvalId}/decision`,async route=>{
   expect(owner).toBe(true);decision=route.request().postDataJSON();given=true;
   await route.fulfill({json:approval(true)});
  });
 };
 await install(context,false);await page.clock.install({time:new Date(now)});
 await signIn(page,PEOPLE.vera);
 const login=await request.post(`${DOUBLE}/api/v1/auth/login`,{data:{login:PEOPLE.vera.login,password:PEOPLE.vera.password}});
 expect(login.ok()).toBe(true);const {access_token:token}=await login.json() as {access_token:string};
 expect((await request.patch(`${DOUBLE}/api/v1/me`,{headers:{Authorization:`Bearer ${token}`},data:{locale,theme:locale==='en'?'light':'dark'}})).ok()).toBe(true);
 await page.goto(`/doc/${DOCS.plan}`);await expect(page.getByRole('textbox',{name:words.documents.page.body,exact:true})).toBeVisible();
 const locked=()=>page.getByRole('button',{name:shared,exact:true});
 await locked().click();
 const card=()=>page.locator('[data-reference-card][data-state="open"]');
 await card().getByRole('textbox').fill('Working together');
 await card().getByRole('button',{name:words.access.request.send,exact:true}).click();
 await expect(card()).toContainText(words.access.request.waiting);
 await expect(card().getByRole('button',{name:words.access.request.cancel,exact:true})).toBeEnabled();
 expect(sent).toEqual({items:[{object:{kind:'document',id:DOCS.private},label:shared,permissions:['doc.view']}],reason:'Working together'});
 await page.screenshot({path:test.info().outputPath(`request-waiting-${locale}.png`),animations:'disabled'});
 const ownerContext=await browser.newContext({baseURL:new URL(page.url()).origin});
 try{
  await install(ownerContext,true);const owner=await ownerContext.newPage();await signIn(owner,PEOPLE.ada);
  await owner.goto(`/doc/${DOCS.plan}`);await expect(owner.getByRole('textbox',{name:english.documents.page.body,exact:true})).toBeVisible();
  await owner.getByRole('link',{name:privateTarget.title,exact:true}).click();
  const ownerCard=owner.locator('[data-reference-card][data-state="open"]');
  await ownerCard.getByRole('button',{name:english.approvals.quick.other,exact:true}).click();
  await ownerCard.getByRole('combobox',{name:english.approvals.access.ends,exact:true}).click();
  await owner.getByRole('option',{name:english.approvals.access.customEnd,exact:true}).click();
  await expect(ownerCard.getByLabel(english.approvals.access.endsOn,{exact:true})).toBeVisible();
  await expect(ownerCard.getByText(english.approvals.access.ends,{exact:true})).toHaveCount(1);
  await owner.screenshot({path:test.info().outputPath(`reference-custom-date-${locale}.png`),animations:'disabled'});
  await owner.keyboard.press('Escape');await owner.getByRole('link',{name:privateTarget.title,exact:true}).click();
  const quick=owner.locator(`[data-access-row="${approvalId}"]`).getByRole('button',{name:english.approvals.quick.always,exact:true}).locator('..');
  await expect(quick.getByRole('button')).toHaveCount(6);
  const granted=owner.waitForResponse(response=>new URL(response.url()).pathname.endsWith(`/approvals/${approvalId}/decision`));
  await quick.getByRole('button').first().click();expect((await granted).ok()).toBe(true);
  expect(decision).toEqual({approve:true,quick_duration:'1d'});
  await dropConnections();
  const readable=page.getByRole('link',{name:privateTarget.title,exact:true});await expect(readable).toBeVisible();
  await readable.click();await expect(card()).toContainText('Current readable description');
  await expect(card().getByRole('button',{name:words.people.reference.open,exact:true})).toBeEnabled();
  await page.screenshot({path:test.info().outputPath(`request-given-${locale}.png`),animations:'disabled'});
  expired=true;
  await page.clock.fastForward(Math.max(1,Date.parse(end)-await page.evaluate(()=>Date.now())+1));
  await expect(locked()).toHaveAttribute('data-reference-state','no-access');
  await expect(card()).toContainText(words.people.reference.noAccess);
  await expect(card()).not.toContainText(privateTarget.title);await expect(card()).not.toContainText('Current readable description');
  await page.screenshot({path:test.info().outputPath(`request-expired-${locale}.png`),animations:'disabled'});
 }finally{await ownerContext.close();}
});
