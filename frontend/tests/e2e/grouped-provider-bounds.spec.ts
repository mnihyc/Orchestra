/** The actual grouped provider picker and its native inspection stay within the viewport. */
import { expect, test } from '@playwright/test';
import { ACME, scenario } from './support';
import { signInScreenRoute } from '../../e2e/layout-fixtures';
const places = [
  { name: 'organization', path: '/org/acme/settings/policies' },
  { name: 'topic', path: `/topic/${ACME.fixLogin}?tab=settings&section=policies` },
] as const;
for (const place of places) for (const locale of ['en','zh-CN'] as const) for (const theme of ['light','dark'] as const) for (const width of [400,1440] as const) {
  test(`grouped provider bounds ${place.name}/${locale}/${theme}/${width}`,async({page},info)=>{
    await scenario();await signInScreenRoute(page,{locale,theme,width});
    await page.goto(place.path);
    const row=page.locator('[data-setting="agent.default_provider_id"]');await expect(row).toBeVisible();
    const combo=row.getByRole('combobox');
    if(await combo.count()===0)await row.locator('[data-setting-editor-trigger]').click();
    await combo.evaluate((element)=>element.scrollIntoView({block:'end'}));
    await combo.click();const list=page.getByRole('listbox').first();await expect(list).toBeVisible();
    for(const name of ['Claude subscription','Codex subscription','Fresh key','OpenAI key','Team Anthropic']) {
      await expect(list.getByRole('option',{name,exact:true})).toBeVisible();
    }
    const count=await list.getByRole('option').count();expect(count).toBeGreaterThan(0);
    const boxes:unknown[]=[];
    for(let index=0;index<count;index+=1){
      const option=list.getByRole('option').nth(index);await option.hover();
      await combo.press('ArrowRight');
      await expect(page.locator('[data-inspection-card]:visible')).toBeVisible();
      await page.evaluate(()=>new Promise<void>((done)=>requestAnimationFrame(()=>requestAnimationFrame(()=>done()))));
      const current=await page.locator('.floating-layer:visible,[data-radix-popper-content-wrapper] > :visible').evaluateAll((layers)=>layers.map((layer)=>{const r=layer.getBoundingClientRect();return {text:layer.getAttribute('aria-label'),left:r.left,top:r.top,right:r.right,bottom:r.bottom};}));
      for(const box of current){expect(box.left).toBeGreaterThanOrEqual(-1);expect(box.top).toBeGreaterThanOrEqual(-1);expect(box.right).toBeLessThanOrEqual(width+1);expect(box.bottom).toBeLessThanOrEqual(901);}
      boxes.push(current);await page.keyboard.press('Escape');
      if(!await list.isVisible())await combo.click();
    }
    await info.attach(`grouped-provider-bounds-${place.name}-${locale}-${theme}-${width}`,{body:JSON.stringify(boxes),contentType:'application/json'});
    await info.attach(`grouped-provider-${place.name}-${locale}-${theme}-${width}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
    // EN: Open organization or topic Settings → Defaults → Default provider, inspect every grouped choice, and keep each surface inside the viewport.
    // 中文：打开组织或主题的设置 → 默认值 → 新线程的提供方，检查每个分组选择，确认浮层始终位于视口内。
  });
}
