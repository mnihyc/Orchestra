/** Resource library controls and figures fit both widths and themes, keep independent page state and add no row reads. */
import { writeFile } from 'node:fs/promises';
import { createInstance } from 'i18next';
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with {type:'json'};
import chinese from '../../src/i18n/zh-CN.json' with {type:'json'};
import { detailAgent } from '../component/resource-detail-fixtures';
import { resources, facts } from './library-fixtures';
import { inventoryRow } from '../doubles/everything-row';
import { scenario, signIn } from './support';

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)for(const width of [1440,400]) {
 test(`library controls, figures and requests (${locale}/${theme}/${width})`,async({page},info)=>{
  await scenario();await page.setViewportSize({width,height:900});await signIn(page);
  const words=createInstance();await words.init({lng:locale,fallbackLng:false,resources:{en:{translation:english},'zh-CN':{translation:chinese}}});const t=words.getFixedT(locale);
  await page.goto('/settings/interface');await page.getByRole('radio',{name:theme==='dark'?'Dark':'Light',exact:true}).click();
  if(locale==='zh-CN'){await page.getByRole('combobox',{name:'Language',exact:true}).click();await page.getByRole('option',{name:'简体中文',exact:true}).click();}
  const reads:string[]=[];page.on('request',(request)=>{if(request.url().includes('/api/v1/'))reads.push(`${request.method()} ${new URL(request.url()).pathname}${new URL(request.url()).search}`);});
  await page.route('**/api/v1/everything?*',async(route)=>{
   const url=new URL(route.request().url());if(url.searchParams.get('resource_details')!=='true'){await route.continue();return;}
   const resource=Object.values(resources).find((r)=>r.kind===url.searchParams.get('kinds'));if(resource===undefined){await route.continue();return;}
   const item=inventoryRow({id:resource.value.id,kind:resource.kind,name:resource.value.name,my_access:'manage',resource,library:facts(resource),library_threads:resource.kind==='provider'?4:undefined});
   await route.fulfill({json:{items:[item],next_cursor:null}});
  });
  const cold:string[][]=[];
  let peopleReads=0;
  await page.route('**/api/v1/resources/library/people',async(route)=>{peopleReads+=1;await route.fulfill({json:{items:[detailAgent.owner],hidden:1,next_cursor:null}});});
  for(const [route,resource]of Object.entries(resources)){
   const before=reads.length;await page.mouse.move(width-2,898);await page.goto(`/resources/${route}`);
   const view=page.getByRole('combobox',{name:t('library.show'),exact:true});await expect(view).toBeVisible();await expect(view).toHaveText(resource.kind==='environment'?t('library.views.viewable'):t('library.views.usable'));
   const bar=page.locator('[data-filter-bar]').first();expect(await bar.getByRole('combobox').first().getAttribute('aria-labelledby')).toBe(await view.getAttribute('aria-labelledby'));
   await expect(page.getByRole('table')).toContainText(resource.value.name);
   if(resource.kind!=='machine')await expect(page.locator('[data-library-counts]').first()).toBeVisible();
   const initial=reads.slice(before);cold.push(initial);expect(initial.filter((path)=>path.startsWith('GET /api/v1/everything?')&&path.includes('resource_details=true'))).toHaveLength(1);
   expect(initial.filter((path)=>path.startsWith('POST /api/v1/resources/library'))).toEqual([]);
   await expect.poll(()=>page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
   const capture=info.outputPath(`${route}-${locale}-${theme}-${width}.png`);await page.screenshot({path:capture,fullPage:true});
   await info.attach(`${route}-${locale}-${theme}-${width}.png`,{path:capture,contentType:'image/png'});
   if(route==='providers'){
    await page.getByRole('button',{name:t('library.given',{number:'3',unit:t('library.units.threads',{count:3})}),exact:true}).click();
    await expect(page.getByText('Visible destination',{exact:true})).toBeVisible();await expect(page.getByText(t('library.more',{number:'2'}),{exact:true})).toBeVisible();
    await page.screenshot({path:info.outputPath(`given-${locale}-${theme}-${width}.png`)});await page.keyboard.press('Escape');
    await page.getByRole('button',{name:t('library.usedTitle'),exact:true}).click();await expect(page.getByText(t('library.recent',{number:'12'}),{exact:true})).toBeVisible();
    await page.getByRole('button',{name:t('library.people',{number:'2',count:2}),exact:true}).click();await expect(page.getByText('Ada Lovelace',{exact:true}).last()).toBeVisible();
    await expect(page.getByText(t('ui.avatarStack.andMore',{count:1}),{exact:true})).toBeVisible();expect(peopleReads).toBe(1);
    await page.screenshot({path:info.outputPath(`used-people-${locale}-${theme}-${width}.png`)});await page.keyboard.press('Escape');
    await expect(page.getByText(t('library.recent',{number:'12'}),{exact:true})).toBeVisible();await page.keyboard.press('Escape');
    await view.click();const options=page.getByRole('option');await expect(options.nth(0)).toHaveText(t('library.views.mine'));await expect(options.nth(1)).toHaveText(t('library.views.managed'));await expect(options.nth(2)).toHaveText(t('library.views.usable'));await expect(options.nth(3)).toHaveText(t('library.views.viewable'));
    await page.getByRole('option',{name:t('library.views.managed'),exact:true}).click();await expect(view).toHaveText(t('library.views.managed'));
    await page.getByRole('button',{name:t('resources.controls.sort'),exact:true}).click();await page.getByRole('combobox',{name:t('everything.sortBy'),exact:true}).click();await page.getByRole('option',{name:t('inventoryChoices.everything.columns.use.name'),exact:true}).click();await expect(page.getByRole('combobox',{name:t('everything.direction'),exact:true})).toHaveText(t('inventoryChoices.everything.directions.desc.name'));await page.keyboard.press('Escape');
   }
  }
  const before=reads.length;await page.goto('/resources/providers');await expect(page.getByRole('combobox',{name:t('library.show'),exact:true})).toHaveText(t('library.views.managed'));
  await page.getByRole('button',{name:t('resources.controls.sort'),exact:true}).click();await expect(page.getByRole('combobox',{name:t('everything.sortBy'),exact:true})).toHaveText(t('inventoryChoices.everything.columns.use.name'));await page.keyboard.press('Escape');
  const warm=reads.slice(before);expect(warm.filter((path)=>path.startsWith('GET /api/v1/everything?')&&path.includes('resource_details=true')).length).toBeLessThanOrEqual(1);
  const traffic=info.outputPath('library-traffic.json');await writeFile(traffic,JSON.stringify({cold,warm},null,2));
  await info.attach('library-traffic',{path:traffic,contentType:'application/json'});
  // The frame's totals are captured for its own budget; figures and the view menu share this one inventory read.
  for(const initial of cold)expect(initial.filter((path)=>path.includes('resource_details=true'))).toHaveLength(1);
 });
}
