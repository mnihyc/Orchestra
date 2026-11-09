/** Own bilingual path: consent starts a new request, and a lost answer replays that confirmed request. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { begin, MAIN, openThread, showView } from './inspector-support';

for(const locale of ['en','zh-CN'] as const) test(`personal read sharing retains its confirmed identity ${locale}`,async({page})=>{
  await begin();await openThread(page);
  if(locale==='zh-CN') {
    await page.goto('/settings/interface');await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();
    await page.getByRole('option',{name:'简体中文',exact:true}).click();await expect(page.locator('html')).toHaveAttribute('lang',locale);
    await page.goto(`/thread/${MAIN}`);
  }
  const word=(key:string)=>caption(`@${key}`,locale);
  await showView(page,'Knowledge');
  await page.getByRole('button',{name:word('threadViews.knowledge.share'),exact:true}).click();
  await page.getByRole('button',{name:`${word('threadViews.knowledge.choose')}: ${word('threadViews.knowledge.chooseAny')}`,exact:true}).click();
  await page.getByRole('textbox',{name:word('threadViews.knowledge.choosePlaceholder'),exact:true}).fill('Web');
  await page.getByRole('dialog',{name:word('threadViews.knowledge.choose'),exact:true}).getByRole('button',{name:/^Web(?:\s|$)/}).first().click();
  const dialog=page.getByRole('dialog',{name:word('access.agents.dialog.title').replace('{{name}}','Web'),exact:true});
  const attempts:{key:string|undefined;body:string|null;confirmation:string|undefined}[]=[];
  let receipt:unknown;
  let creates=0;
  await page.route('**/api/v1/share/agents',async route=>{
    const request=route.request();
    attempts.push({key:request.headers()['idempotency-key'],body:request.postData(),confirmation:request.headers()['orch-owner-reads-confirmed']});
    if(attempts.length===1) {
      await route.fulfill({status:409,json:{type:'orch:error:conflict',code:'conflict',title:'Confirm sharing',status:409,detail:'Confirm sharing.',request_id:'disclosure',owner_reads:{count:2,threads:1,confirmation:'reviewed-personal-results'}}});return;
    }
    expect(attempts.at(-1)?.confirmation).toBe('reviewed-personal-results');
    if(attempts.length===2) {
      const response=await route.fetch();expect(response.status()).toBe(201);creates++;
      const reply=await response.json() as {shares:{id:string;to:{id:string;kind:string};mode:string;input:{id:string}|null}[]};
      receipt={projection:'receipt',shares:reply.shares.map(share=>({id:share.id,to:{id:share.to.id,kind:share.to.kind},mode:share.mode,created:true,input_id:share.input?.id??null}))};
      await route.abort('connectionfailed');return;
    }
    await route.fulfill({status:201,json:receipt});
  });
  const who=word('access.agents.dialog.button.ofAgents').replace('{{name}}','Main thread');
  const what=word('access.agents.dialog.button.this').replace('{{name}}','Web');
  const submit=dialog.getByRole('button',{name:word('access.agents.dialog.button.read').replace('{{who}}',who).replace('{{what}}',what),exact:true});
  await expect(submit).toBeEnabled();await submit.click();
  const review=page.getByRole('alertdialog',{name:word('access.ownerReads.title'),exact:true});
  await expect(review).toContainText(word('access.ownerReads.notice').replace('{{count}}','2'));
  expect(creates).toBe(0);expect(attempts).toHaveLength(1);
  await review.getByRole('button',{name:word('access.ownerReads.confirm'),exact:true}).click();
  await expect(dialog.getByRole('alert')).toBeVisible();
  expect(creates).toBe(1);
  await submit.focus();await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0);expect(attempts).toHaveLength(3);expect(creates).toBe(1);
  expect(attempts[1]?.key).not.toBe(attempts[0]?.key);
  expect(attempts[1]?.body).toBe(attempts[0]?.body);
  expect(attempts[2]).toEqual(attempts[1]);
  const body=JSON.parse(attempts[0]?.body??'null') as {client_id:string};
  expect(body.client_id).toBe(attempts[0]?.key);
  // EN: Thread → Knowledge → Share → Choose Web → Share → read the notice → Share these results → retry the lost reply.
  // 中文：线程 → 知识 → 共享 → 选择 Web → 共享 → 阅读提示 → 共享这些结果 → 重试丢失的响应。
  // Fixture transport proves browser identity retention; the adjacent API case proves real native once-only creation.
});
