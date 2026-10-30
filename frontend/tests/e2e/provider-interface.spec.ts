/** The real provider page/wizard stay compact and reachable at both widths, languages and themes. */
import { expect,test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario,signIn } from './support';

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)test(`provider interface ${locale}/${theme}`,async({page},info)=>{
  await scenario();await page.emulateMedia({colorScheme:theme});await signIn(page);
  if(locale==='zh-CN'){
    await page.goto('/settings/interface');await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();
    await page.getByRole('option',{name:'简体中文',exact:true}).click();
  }
  const words=locale==='en'?english:chinese;
  await page.goto('/resources/providers');
  const list=page.getByRole('table',{name:words.providers.list.label});
  const first=list.locator('tbody a[href*="/resources/providers/"]').first();
  await expect(first).toBeVisible();const href=await first.getAttribute('href');
  if(href===null)throw new Error('the provider row has no destination');
  const base=href.split('?')[0] ?? href;
  const reads:string[]=[];
  page.on('request',(request)=>{if(request.method()==='GET' && request.url().includes('/api/v1/'))reads.push(new URL(request.url()).pathname);});
  await page.goto(base+'?tab=overview');
  const tabs=page.getByRole('tablist',{name:words.providers.tabs.label});
  await expect(tabs).toBeVisible();
  expect(reads.filter((path)=>/\/(suggestions|threads|sessions|settings|provider-setup|usage)$/.test(path))).toEqual([]);
  // A bare resource URL opens its own Overview rather than another provider's saved tab.
  await page.goto(base+'?tab=settings');await expect(page.getByRole('tab',{name:words.providers.tabs.settings,exact:true})).toHaveAttribute('data-state','active');
  await page.goto(base);await expect(page.getByRole('tab',{name:words.providers.tabs.overview,exact:true})).toHaveAttribute('data-state','active');
  for(const width of [1440,400]){
    await page.setViewportSize({width,height:1000});
    await expect(tabs.getByRole('tab')).toHaveCount(6);
    for(const tab of await tabs.getByRole('tab').all())await expect(tab).toBeVisible();
    await expect(tabs.getByRole('button',{name:/More|更多/})).toHaveCount(0);
    expect(await page.locator('[data-unread-members]').count()).toBeLessThanOrEqual(1);
    await expect(page.getByRole('button',{name:words.resourceCreation.providers,exact:true})).toHaveCount(0);
    await info.attach(`provider-${width}-${locale}-${theme}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
  }
  // Creation belongs to the list; the existing provider keeps only its own actions.
  await page.goto('/resources/providers');
  await page.locator('[data-resource-kind="providers"]').getByRole('button',{name:words.providers.list.add,exact:true}).click();
  const dialog=page.getByRole('dialog',{name:words.providers.add.title});
  await expect(dialog.getByRole('combobox',{name:words.providerCreation.reach})).toBeVisible();
  await expect(dialog.getByRole('combobox',{name:words.providerCreation.examples})).toBeVisible();
  const examples=dialog.getByRole('combobox',{name:words.providerCreation.examples});
  const reach=dialog.getByRole('combobox',{name:words.providerCreation.reach});
  expect(await examples.evaluate((element)=>Array.from(element.closest('[role="dialog"]')?.querySelectorAll('[role="combobox"]') ?? [])[0]===element)).toBe(true);
  await expect(dialog.getByText(words.resources.endpoint.noAgent,{exact:true})).toHaveCount(0);
  await dialog.getByRole('combobox',{name:words.resources.endpoint.protocol}).click();
  await expect(page.getByRole('listbox')).toBeVisible();
  await expect(page.getByRole('listbox').getByRole('option')).toHaveCount(3);
  for(const option of await page.getByRole('listbox').getByRole('option').all())await expect(option).not.toHaveAttribute('aria-describedby',/.+/);
  await page.keyboard.press('Escape');
  await dialog.getByRole('combobox',{name:words.resources.home.label}).click();
  await expect(page.getByRole('listbox')).toBeVisible();
  for(const option of await page.getByRole('listbox').getByRole('option').all())await expect(option).not.toHaveAttribute('aria-describedby',/.+/);
  await page.keyboard.press('Escape');
  const address=dialog.getByLabel(words.resources.endpoint.address,{exact:true});
  await expect(address).toBeVisible();
  const padding=await address.evaluate((input)=>({left:Number.parseFloat(getComputedStyle(input).paddingLeft),right:Number.parseFloat(getComputedStyle(input).paddingRight)}));
  expect(padding.left).toBeGreaterThanOrEqual(8);expect(padding.right).toBeGreaterThanOrEqual(8);
  await info.attach(`provider-create-400-${locale}-${theme}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
  await reach.click();await page.getByRole('option',{name:words.providerCreation.native,exact:true}).click();
  await dialog.getByRole('combobox',{name:words.providerCreation.program}).click();
  await expect(page.getByRole('listbox')).toBeVisible();
  for(const name of ['Codex','Claude Code'])expect(await page.getByRole('listbox').getByRole('option',{name:new RegExp(name)}).count()).toBeGreaterThan(0);
  await page.keyboard.press('Escape');
  await expect(dialog.getByRole('button',{name:words.resourceCreation.agents,exact:true})).toBeVisible();
});
