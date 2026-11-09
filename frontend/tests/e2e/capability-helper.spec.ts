/** EN: Open a delegated image call, inspect its private default, follow its ordinary helper, and return by its source link.
 * 中文：展开委托图像调用，先查看默认私有范围，再打开普通助手线程，并通过来源链接返回。
 * Permission/custody/billing are backend deciding cases; these are the actual browser links and cards. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { toolCall, userMessage } from '../doubles/thread-rows';
import { ADA, GRACE } from '../doubles/thread-scenes';
import type { ReferencePreview } from '../../src/generated/api-types';
import { ACME, DOUBLE, configureThread, emit, scenario, signIn } from './support';

for (const locale of ['en','zh-CN'] as const) for (const theme of ['light','dark'] as const) for (const width of [400,1440]) {
  test(`a delegated image keeps its private ordinary helper link ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await configureThread(ACME.mainThread,{reset:true});
    await configureThread(ACME.depsThread,{reset:true});
    const appearance=await fetch(`${DOUBLE}/__double/first-steps/appearance`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({locale,theme}),signal:AbortSignal.timeout(10_000)});
    expect(appearance.ok).toBe(true);
    const words=locale==='en'?english:chinese;
    const helper={thread_id:ACME.depsThread,actor_id:ADA.id,actor:ADA,actor_name:ADA.display_name,
      program_label:{en:'Codex on ChatGPT','zh-CN':'Codex（ChatGPT 登录）'},requester_thread_id:ACME.mainThread};
    await emit(ACME.mainThread,{items:[toolCall('delegated-image',{kind:'orch',tool:'image_generation',arguments:{prompt:'An orchard',output_paths:['orchard.png']},result:{state:'running',helper}},{title:'image_generation'})]});
    await emit(ACME.depsThread,{items:[userMessage('helper-origin',`[Requesting thread](/thread/${ACME.mainThread})`,{author:ADA})]});
    await page.setViewportSize({width,height:900});
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    const step=page.locator('[data-step="delegated-image"]');
    await step.locator('[data-tool-row] > button[aria-expanded]').click();
    const row=step.locator('[data-capability-helper]');
    await expect(row).toContainText(helper.program_label[locale]);
    await expect(row).toContainText(ADA.display_name);
    await expect(row).toContainText(locale==='en'?'Private to':'默认仅');
    await expect(row).toContainText(locale==='en'?'by default':'可见');
    const open=row.getByRole('link').first();
    await expect(open).toHaveAttribute('href',`/thread/${ACME.depsThread}`);
    await open.focus();
    await page.keyboard.press('Enter');
    const card=page.locator('[data-reference-card]:visible').last();
    await card.getByRole('button',{name:words.people.reference.open,exact:true}).click();
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    const source=page.locator(`[data-item="helper-origin"] a[href="/thread/${ACME.mainThread}"]`).first();
    await expect(source).toHaveAttribute('href',`/thread/${ACME.mainThread}`);
    const [returned]=await Promise.all([page.context().waitForEvent('page'),source.click({modifiers:['ControlOrMeta']})]);
    await expect(returned.locator('[data-step="delegated-image"]')).toBeVisible();
    await returned.reload();
    await expect(returned.locator('[data-step="delegated-image"]')).toBeVisible();
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);
    const path=info.outputPath(`capability-helper-${locale}-${theme}-${width}.png`);
    await page.screenshot({path,fullPage:true,animations:'disabled'});
    await info.attach('capability-helper',{path,contentType:'image/png'});
  });
}

// The reader can read the requesting item; the ordinary helper has a separate current access result.
for (const locale of ['en','zh-CN'] as const) for (const theme of ['light','dark'] as const) for (const width of [400,1440]) {
  for (const state of ['private','deleted'] as const) {
    test(`a ${state} delegated helper keeps its audience visible before opening ${locale}/${theme}/${width}`, async ({ page }, info) => {
      await scenario();
      await configureThread(ACME.mainThread,{reset:true});
      const appearance=await fetch(`${DOUBLE}/__double/first-steps/appearance`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({locale,theme}),signal:AbortSignal.timeout(10_000)});
      expect(appearance.ok).toBe(true);
      const words=locale==='en'?english:chinese;
      await page.route('**/api/v1/references/preview?**', async (route) => {
        if (new URL(route.request().url()).searchParams.get('id')!==ACME.depsThread) { await route.continue(); return; }
        const body:ReferencePreview={kind:'node',id:ACME.depsThread,node_kind:'thread',title:'',description:'',place:null,updated_at:null,updated_by:null,part:null,file:null,
          unavailable:{deleted:state==='deleted',object:{kind:'thread',id:ACME.depsThread},permission:'thread.view',requestable:false,pending:null}};
        await route.fulfill({json:body});
      });
      const helper={thread_id:ACME.depsThread,actor_id:GRACE.id,actor:GRACE,actor_name:GRACE.display_name,
        program_label:{en:'Codex on ChatGPT','zh-CN':'Codex（ChatGPT 登录）'},requester_thread_id:ACME.mainThread};
      await emit(ACME.mainThread,{items:[toolCall(`delegated-${state}`,{kind:'orch',tool:'image_generation',arguments:{prompt:'An orchard',output_paths:['orchard.png']},result:{state:'running',helper}},{title:'image_generation'})]});
      await page.setViewportSize({width,height:900});
      await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
      await signIn(page);
      await page.goto(`/thread/${ACME.mainThread}`);
      const step=page.locator(`[data-step="delegated-${state}"]`);
      await step.locator('[data-tool-row] > button[aria-expanded]').click();
      const row=step.locator('[data-capability-helper]');
      await expect(row).toContainText(helper.program_label[locale]);
      await expect(row).toContainText(GRACE.display_name);
      await expect(row).toContainText(locale==='en'?'Private to':'默认仅');
      const name=state==='deleted'?words.people.reference.deleted:words.timeline.capabilityHelper;
      const chip=row.getByRole('button',{name,exact:true});
      await expect(chip).toHaveAttribute('data-reference-state',state==='deleted'?'deleted':'no-access');
      await chip.focus();
      await page.keyboard.press('Enter');
      const card=page.locator('[data-reference-card][data-state="open"]');
      if (state==='deleted') {
        await expect(card).toContainText(words.people.reference.deleted);
        await expect(card.getByRole('button',{name:words.people.reference.open,exact:true})).toBeDisabled();
      } else {
        await expect(card).toContainText(words.people.reference.noAccess);
        await expect(card.getByRole('button',{name:words.people.reference.open,exact:true})).toHaveCount(0);
      }
      await expect(card).not.toContainText('Current helper description');
      await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}`));
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBe(true);
      const path=info.outputPath(`capability-helper-${state}-${locale}-${theme}-${width}.png`);
      await page.screenshot({path,fullPage:true,animations:'disabled'});
      await info.attach(`capability-helper-${state}`,{path,contentType:'image/png'});
      await page.keyboard.press('Escape');
      await expect(card).toHaveCount(0);
      await expect(chip).toBeFocused();
    });
  }
}
