/** Own click paths: the inherited option and the server's informed-sharing confirmation in both languages. */
import { expect, test } from '@playwright/test';
import type { ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn, supplyDefaults } from './support';

for (const locale of ['en', 'zh-CN'] as const) {
  test(`personal reads are off by default and keep the inherited lock ${locale}`, async ({ page }) => {
    await scenario();
    const definition: SettingDef = {
      key:'orch_tools.owner_reads',title:'Personal read tools',description:'Read with the owner’s current access, including when shared.',
      group:'agent',class:'policy',scopes:['system','org','project','topic','thread'],merge:'replace',
      schema:{type:'boolean'},default:false,example:false,text_key:null,keywords:[],
    };
    let current:ResolvedSetting={key:definition.key,value:false,own:null,writable:true,locked_by:null,source:{scope:'default',node:null},parts:[]};
    const writes:unknown[]=[];
    await page.route('**/api/v1/settings/registry',route=>route.fulfill({json:[definition]}));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`,route=>route.fulfill({json:[current]}));
    await supplyDefaults(page, ACME.research, { registry: () => [definition], settings: () => [current] });
    await page.route(`**/api/v1/nodes/${ACME.research}/settings/${definition.key}`,async route=>{
      const body=route.request().postDataJSON() as {value:boolean;expected_revision:number};
      writes.push(body);current={...current,value:body.value,own:{value:body.value,revision:1,locked:false}};
      await route.fulfill({json:current});
    });
    await signIn(page);
    if(locale==='zh-CN') {await page.goto('/settings/interface');await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();await page.getByRole('option',{name:'简体中文',exact:true}).click();}
    const words=locale==='en'?english:chinese;
    await page.goto('/org/acme.research/settings/defaults');
    const row=page.locator('[data-setting="orch_tools.owner_reads"]');
    await expect(row).toContainText(words.settingRegistry.orch_tools.owner_reads.title);
    await expect(row).toContainText(words.settingRegistry.orch_tools.owner_reads.description);
    const value=row.getByRole('switch',{name:words.settingRegistry.orch_tools.owner_reads.title,exact:true});
    await expect(value).not.toBeChecked();expect(writes).toHaveLength(0);
    // Discrete settings save in place; the separate Save action was removed.
    await value.check();
    await expect.poll(()=>writes.length).toBe(1);expect(writes[0]).toMatchObject({value:true,expected_revision:0});
    await expect(value).toBeChecked();
    await expect(row.getByRole('status')).toContainText(words.settings.compact.saved);
    current={...current,writable:false,locked_by:{scope:'org',node:{id:ACME.org,kind:'org',name:'Acme',redacted:false}}};
    await page.reload();await expect(row).toContainText(words.settingRegistry.orch_tools.owner_reads.title);
    await expect(value).toHaveCount(0);
    await expect(row.getByRole('button',{name:words.common.save,exact:true})).toHaveCount(0);
    expect(writes).toHaveLength(1);
    // EN: Organization → Settings → Defaults → Personal read tools → On. An inherited lock removes the editor.
    // 中文：组织 → 设置 → 默认值 → 个人读取工具 → 开启；继承锁定后不显示编辑控件。
  });

  for(const results of [0,2]) test(`sharing personal history requires an informed act only with results ${locale}/${results}`,async({page})=>{
    await scenario();await signIn(page);
    if(locale==='zh-CN') {await page.goto('/settings/interface');await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();await page.getByRole('option',{name:'简体中文',exact:true}).click();}
    const words=locale==='en'?english:chinese;
    const requests:{body:unknown;confirmation:string|undefined}[]=[];
    let applied=0;
    await page.route(`**/api/v1/nodes/${ACME.mainThread}/permissions`,async route=>{
      if(route.request().method()!=='PATCH') {await route.fallback();return;}
      const confirmation=route.request().headers()['orch-owner-reads-confirmed'];
      requests.push({body:route.request().postDataJSON(),confirmation});
      if(results>0&&confirmation!=='current-count-and-intent') {
        await route.fulfill({status:409,json:{type:'orch:error:conflict.confirmation_required',code:'conflict.confirmation_required',title:'Confirm sharing',status:409,detail:'Confirm the disclosure.',request_id:'sharing-review',owner_reads:{count:results,threads:1,confirmation:'current-count-and-intent'}}});return;
      }
      applied++;await route.continue();
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('[data-thread-header] [data-thread-more]').click();
    await page.getByRole('menuitem', { name: words.thread.header.shareThread, exact: true }).click();
    const sharing=page.getByRole('dialog').filter({has:page.getByRole('combobox',{name:words.access.add.label,exact:true})});
    await sharing.getByRole('combobox',{name:words.access.add.label,exact:true}).fill('gra');
    await page.getByRole('option',{name:/Grace Hopper/}).click();
    await sharing.getByRole('button',{name:words.access.add.button,exact:true}).click();
    const confirmation=page.getByRole('alertdialog',{name:words.access.ownerReads.title,exact:true});
    if(results===0) {await expect.poll(()=>applied).toBe(1);await expect(confirmation).toHaveCount(0);}
    else {
      await expect(confirmation).toContainText(words.access.ownerReads.notice.replace('{{count}}',String(results)));
      expect(applied).toBe(0);
      await confirmation.getByRole('button',{name:words.common.cancel,exact:true}).click();
      expect(applied).toBe(0);
      await sharing.getByRole('button',{name:words.access.add.button,exact:true}).click();
      await expect(confirmation).toBeVisible();
      await confirmation.getByRole('button',{name:words.access.ownerReads.confirm,exact:true}).click();
      await expect.poll(()=>applied).toBe(1);
      expect(requests.at(-1)?.confirmation).toBe('current-count-and-intent');
      expect(requests.at(-1)?.body).toEqual(requests.at(-2)?.body);
    }
    // EN: Thread → Share → Add Grace Hopper → Add → read disclosure → Cancel, then Add → Share these results.
    // 中文：线程 → 共享 → 添加 Grace Hopper → 添加 → 阅读提示 → 取消；再次添加 → 共享这些结果。
    // The native API cases also prove that confirmed history is visible and personal reads remain enabled.
  });
}
