/** Own click path: existing Call recovery setting → capability purpose → exact route → save/reopen. */
import { expect,test } from '@playwright/test';
import english from '../../src/i18n/en.json' with {type:'json'};
import chinese from '../../src/i18n/zh-CN.json' with {type:'json'};
import { RUNS_ON } from '../doubles/fixtures';
import { ACME,scenario,signIn } from './support';
const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)for(const width of [1440,400]) {
  test(`capability recovery rule click path ${locale}/${theme}/${width}`,async({page,request},info)=>{
    await scenario();await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    const candidate={provider_id:RUNS_ON.openAiKey,model:'',params:{},route:{route:'web_search.service-api',offering:{provider:RUNS_ON.openAiKey,offering:'public-search'},config:{}}};
    const value=[{id:'web-fallback',when:{kind:'unavailable',purpose:'web_search'},steps:[{type:'failover',candidate},{type:'wait',until:'person'}]}];
    const seeded=await request.post(`${DOUBLE}/__double/day100/setting`,{data:{node:ACME.web,key:'calls.rules',value}});expect(seeded.ok()).toBe(true);
    const appearance=await request.post(`${DOUBLE}/__double/day100/appearance`,{data:{username:'ada',locale,theme}});expect(appearance.ok()).toBe(true);
    await signIn(page);
    const catalogue=locale==='en'?english:chinese,words=catalogue.calls;
    await page.goto(`/project/${ACME.web}/settings?section=policies&setting=calls.rules`);
    const setting=page.locator('[data-setting="calls.rules"]');await expect(setting).toBeVisible();
    await setting.locator('[data-setting-editor-trigger]').click();
    await page.getByRole('button',{name:/→/}).first().click();
    const editor=page.getByRole('dialog',{name:words.editRule,exact:true});
    await expect(editor.getByRole('combobox',{name:words.when,exact:true})).toContainText(words.kinds.unavailable);
    await editor.getByRole('combobox',{name:words.purpose,exact:true}).click();
    await page.getByRole('option',{name:words.purposes.page_fetch,exact:true}).click();
    await editor.getByRole('combobox',{name:words.route,exact:true}).click();
    await page.getByRole('option',{name:locale==='en'?'Provider API':'提供方 API',exact:true}).click();
    await editor.getByLabel(words.offering,{exact:true}).fill('public-fetch');
    await expect(editor.getByRole('combobox',{name:words.destination,exact:true})).toContainText(words.capabilityDestination);
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    await info.attach(`capability-rule-${locale}-${theme}-${width}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
    const written=page.waitForRequest((request)=>request.method()==='PUT'&&new URL(request.url()).pathname.endsWith('/settings/calls.rules'));
    const accepted=page.waitForResponse((response)=>response.request().method()==='PUT'&&new URL(response.url()).pathname.endsWith('/settings/calls.rules'));
    await editor.getByRole('button',{name:catalogue.common.save,exact:true}).click();
    const body=(await written).postDataJSON() as {value:typeof value};
    expect(body.value[0]?.when.purpose).toBe('page_fetch');
    expect(body.value[0]?.steps[0]?.candidate?.route.route).toBe('page_fetch.service-api');
    expect(body.value[0]?.steps[0]?.candidate?.route.offering.offering).toBe('public-fetch');
    expect((await accepted).status()).toBe(200);
    await expect(editor).not.toBeVisible();
    await page.reload();
    await setting.locator('[data-setting-editor-trigger]').click();
    await page.getByRole('button',{name:/→/}).first().click();
    await expect(editor.getByRole('combobox',{name:words.purpose,exact:true})).toContainText(words.purposes.page_fetch);
    await expect(editor.getByLabel(words.offering,{exact:true})).toHaveValue('public-fetch');
    // EN: Open Call recovery, edit Capability unavailable → Page fetch, select Provider API, name its service, save.
    // 中文：打开“调用恢复”，编辑“能力不可用→网页抓取”，选择“提供方 API”，填写服务并保存。
  });
}
