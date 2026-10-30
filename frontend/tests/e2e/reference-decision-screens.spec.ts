/** Shared access decisions and held messages on the production Inbox, approvals and native thread screens. */
import {expect,test,type APIRequestContext,type Locator} from '@playwright/test';
import {ACME,emit,scenario,signIn} from './support.ts';
import {askAccess,notify} from './inbox-support.ts';
import {id,me,userRef} from '../doubles/fixtures.ts';
import {threadInput} from '../doubles/thread-rows.ts';
import type {Approval,ThreadInput} from '../../src/generated/api-types';

const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT??'41871'}`;
const DAY_MS=24*60*60*1000;
async function profile(request:APIRequestContext,theme:'light'|'dark') {
 const changed=await request.post(`${DOUBLE}/__double/notifications/me`,{data:{user:'ada',locale:'en',theme}});
 expect(changed.ok()).toBe(true);
}
async function compact(button:Locator,frame:Locator) {
 const [control,box]=await Promise.all([button.boundingBox(),frame.boundingBox()]);
 expect(control).not.toBeNull();expect(box).not.toBeNull();
 if(control===null||box===null)throw new Error('A decision control has no bounds');
 expect(control.width).toBeLessThan(box.width/2);
}

for(const width of [400,1440])for(const theme of ['light','dark'] as const){
 test(`Inbox access decision and custom date ${width}px/${theme}`,async({page,request})=>{
  await scenario();await profile(request,theme);await page.setViewportSize({width,height:1000});
  const pending=await askAccess({requester:'grace',title:'Shared rollout plan',thread_id:ACME.mainThread,
   node:{id:ACME.mainThread,kind:'thread',name:'Main thread',redacted:false},
   object:{id:ACME.mainThread,kind:'thread',name:'Main thread'},permissions:['thread.view'],reason:'Review the plan together'});
  await notify({kind:'access.requested',title:'Shared rollout plan',thread_id:ACME.mainThread,node:ACME.mainThread,approval_id:pending.id,url:'/approvals'});
  await signIn(page);await page.goto('/inbox');await expect(page.locator('html')).toHaveAttribute('data-theme',theme);
  const notice=page.locator('li[data-kind="access.requested"]');await expect(notice).toBeVisible();
  await page.screenshot({path:test.info().outputPath(`inbox-pending-${width}-${theme}.png`),animations:'disabled'});
  await notice.locator('a[href*="/approvals"]').first().click();
  const card=page.locator(`[data-focused-approval="${pending.id}"]`);await expect(card).toBeVisible();
  const always=card.getByRole('button',{name:'Always',exact:true});await expect(always).toBeEnabled();
  await page.screenshot({path:test.info().outputPath(`inbox-grant-${width}-${theme}.png`),animations:'disabled'});
  await card.getByRole('button',{name:'Other…',exact:true}).click();
  await card.getByRole('combobox',{name:'Ends',exact:true}).click();
  await page.getByRole('option',{name:'Another date',exact:true}).click();
  await expect(card.getByText('Ends',{exact:true})).toHaveCount(1);
  await card.getByLabel('Ends on',{exact:true}).fill('2030-11-02T12:30');
  await page.screenshot({path:test.info().outputPath(`inbox-custom-date-${width}-${theme}.png`),animations:'disabled'});
  const at=new Date(Date.now()-60_000).toISOString();
  const settled=await request.post(`${DOUBLE}/__double/notifications/approval/settle`,{data:{id:pending.id,by:'ada',state:'approved',at,decision:{approve:true,expires_at:new Date(Date.parse(at)+7*DAY_MS).toISOString()}}});
  expect(settled.ok()).toBe(true);
  await page.goto('/inbox');await expect(notice).toContainText('for 7 days');
  await expect(notice.locator('time[datetime="'+at+'"]')).toBeVisible();
  await expect(notice).toHaveAttribute('data-unread','false');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  await page.screenshot({path:test.info().outputPath(`inbox-settled-${width}-${theme}.png`),animations:'disabled'});
 });

 test(`A held input keeps its author and compact Run it ${width}px/${theme}`,async({page,context,request})=>{
  await scenario();await profile(request,theme);await page.setViewportSize({width,height:1000});
  const inputId=id(0x73,1),approvalId=id(0x73,2),author=userRef(me(2,'grace','Grace Hopper')),actor=userRef(me(1,'ada','Ada Lovelace'));
  const input:ThreadInput={...threadInput(inputId,'The original author asks for this work',{author,position:'1',detail:'model_use_required'}),thread_id:ACME.mainThread,approval_id:approvalId,decision:null};
  await emit(ACME.mainThread,{inputs:[input]});let decided=false;
  const approval=():Extract<Approval,{kind:'input'}>=>({id:approvalId,kind:'input',state:decided?'approved':'pending',title:'A message waits for model use',
   object:{kind:'input',id:inputId,name:'Held message'},node:null,requester:{user:author},request:{input_id:inputId,revision:1,content:input.content},
   required_permission:'thread.interact',can_decide:!decided,approver_scope:null,batch_id:null,thread_id:ACME.mainThread,item_id:null,
   created_at:new Date().toISOString(),decided_at:null,decided_by:null,decided_automatically:false,decision:decided?{run:true}:null,expires_at:null});
  await context.route(`**/api/v1/approvals/${approvalId}`,route=>route.fulfill({json:approval()}));
  await context.route(`**/api/v1/threads/${ACME.mainThread}/inputs/${inputId}/run`,async route=>{
   expect(route.request().method()).toBe('POST');decided=true;const at=new Date().toISOString();
   const reply:ThreadInput={...input,state_detail:null,run_as:actor,decision:{action:'run',state:'approved',decided_by:actor,decided_at:at}};
   const seq=await emit(ACME.mainThread,{inputs:[reply]});await route.fulfill({json:{...reply,seq_updated:seq}});
  });
  await signIn(page);await page.goto(`/approvals?approval=${approvalId}`);
  const card=page.locator(`[data-focused-approval="${approvalId}"]`);await expect(card).toBeVisible();
  await compact(card.getByRole('button',{name:'Run it',exact:true}),card);
  await page.screenshot({path:test.info().outputPath(`held-approval-${width}-${theme}.png`),animations:'disabled'});
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-queue]')).toBeVisible();
  const row=page.locator(`[data-queued="${inputId}"]`);
  if(!await row.isVisible())await page.locator('[data-queue] > div > button[aria-expanded="false"]').click();
  await expect(row).toContainText('Grace Hopper');
  expect(await row.getByText('Grace Hopper',{exact:true}).evaluate(element=>element.scrollWidth<=element.clientWidth+1)).toBe(true);
  const run=row.getByRole('button',{name:/Run message .* on your access/});await expect(run).toBeEnabled();await compact(run,row);
  await page.screenshot({path:test.info().outputPath(`held-thread-${width}-${theme}.png`),animations:'disabled'});
  await run.click();await expect(row.getByRole('status')).toContainText('Ada Lovelace');
  await expect(row).toContainText('Grace Hopper');await expect(run).toHaveCount(0);
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  await page.screenshot({path:test.info().outputPath(`held-run-${width}-${theme}.png`),animations:'disabled'});
 });
}
