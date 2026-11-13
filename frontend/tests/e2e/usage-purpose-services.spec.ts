/** Usage follows factual purposes/offerings and keeps unknown service readings visible. */
import {expect,test} from '@playwright/test';
import english from '../../src/i18n/en.json' with {type:'json'};
import chinese from '../../src/i18n/zh-CN.json' with {type:'json'};
import {scenario,signIn} from './support.ts';
import {PLACES,useService} from './usage-support.ts';

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) test.describe(`${locale}/${theme}`,()=>{
  // Docked bars leave the desktop table its measured md room; 400px still checks folded cells.
  test.use({viewport:{width:1920,height:900}});
  test('purpose and offering controls retain scope, native units and the copied price',async({page},info)=>{
    await scenario();await page.emulateMedia({colorScheme:theme});await signIn(page);
    if(locale==='zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();
      await page.getByRole('option',{name:'简体中文',exact:true}).click();
    }
    const words=locale==='en'?english:chinese;
    const scope=`node:${PLACES.web}`;
    // Filters replace their current history entry. Establish two real scoped report
    // visits so Back/Forward checks that report history rather than returning to sign-in.
    await page.goto(`/usage?scope=${scope}&view=requests`);
    await expect(page.getByRole('tab',{name:words.usagePerf.viewNames.requests,exact:true})).toHaveAttribute('aria-selected','true');
    await page.goto(`/usage?scope=${scope}&view=usage`);
    const figures=page.getByRole('list',{name:words.usage.figures.label,exact:true});
    await expect(figures).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('lang',locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme',theme);
    await info.attach('usage-before-services-desktop',{body:await figures.screenshot(),contentType:'image/png'});
    await page.setViewportSize({width:400,height:900});
    await info.attach('usage-before-services-narrow',{body:await figures.screenshot(),contentType:'image/png'});
    await page.setViewportSize({width:1920,height:900});
    await useService({offering:'studio-images'});
    await useService({offering:'studio-images',unknown:true});
    await page.reload();
    await page.getByRole('tab',{name:words.usage.tables.by.purpose,exact:true}).click();
    const purposes=page.getByRole('table',{name:words.usage.tables.by.purpose,exact:true});
    await purposes.getByRole('button',{name:words.usage.purposes.image_generation,exact:true}).click();
    await expect.poll(()=>new URL(page.url()).searchParams.get('purpose')).toBe('image_generation');
    await expect(figures.getByRole('listitem').filter({hasText:words.usage.serviceUnits.title})).toContainText(locale==='en'?'2 images · 1 unreported':'2 张图像 · 1 次未报告');
    await expect(figures).toContainText(locale==='en'?'1 unpriced request':'1 次请求未计价');
    await page.getByRole('tab',{name:words.usage.tables.by.offering,exact:true}).click();
    const offerings=page.getByRole('table',{name:words.usage.tables.by.offering,exact:true});
    const selected=page.waitForResponse((response)=>{
      const url=new URL(response.url());
      return response.request().method()==='GET' && url.pathname==='/api/v1/usage'
        && url.searchParams.get('scope')===scope && url.searchParams.get('purpose')==='image_generation'
        && url.searchParams.get('service')==='studio-images';
    });
    await offerings.getByRole('button',{name:'studio-images',exact:true}).click();
    const filtered=await selected;expect(filtered.status()).toBe(200);
    const address=new URL(page.url()).searchParams;
    expect(address.get('scope')).toBe(scope);expect(address.get('service')).toBe('studio-images');expect(address.has('model')).toBe(false);
    const chart=page.getByRole('region',{name:words.usage.sections.overTime,exact:true});
    for(const dimension of ['offering','purpose'] as const) {
      const split=page.waitForResponse((response)=>{
        const url=new URL(response.url());
        return response.request().method()==='GET' && url.pathname==='/api/v1/usage'
          && url.searchParams.get('series_by')===dimension && url.searchParams.get('service')==='studio-images';
      });
      await chart.getByRole('combobox',{name:words.usage.chart.splitBy,exact:true}).click();
      await page.getByRole('option',{name:words.usagePerf.choices.splits[dimension].name,exact:true}).click();
      expect((await split).status()).toBe(200);
    }
    const records=page.getByRole('table',{name:words.usage.records.title,exact:true});
    await expect(records.getByRole('row')).toHaveCount(3);
    const known=records.getByRole('row').filter({hasText:locale==='en'?'2 images':'2 张图像'});
    const unknown=records.getByRole('row').filter({hasText:locale==='en'?'– images':'– 张图像'});
    await expect(known).toHaveCount(1);await expect(unknown).toHaveCount(1);
    await expect(records.getByRole('columnheader',{name:words.usagePerf.choices.splits.offering.name,exact:true})).toBeVisible();
    await known.getByRole('button',{name:words.usage.records.aboutRecord,exact:true}).click();
    const detail=page.getByRole('dialog',{name:words.usage.records.aboutRecord,exact:true});
    await expect(detail).toContainText(words.usage.purposes.image_generation);
    await expect(detail).toContainText(locale==='en'?'0.01 credits / image':'0.01 点数 / 张图像');
    await expect(detail).toContainText(words.usage.serviceUnits.applied);
    await expect(detail).not.toContainText(words.usageRequest.uncached);
    await page.keyboard.press('Escape');await expect(detail).not.toBeVisible();
    await info.attach('usage-services-desktop',{body:await page.screenshot(),contentType:'image/png'});
    await page.setViewportSize({width:400,height:900});
    await expect(known).toContainText('studio-images');
    await expect(unknown).toContainText(locale==='en'?'– images':'– 张图像');
    const narrowed=page.getByRole('group',{name:words.usage.chip.list,exact:true});
    await expect(narrowed).toContainText('studio-images');await expect(narrowed).toContainText(words.usage.purposes.image_generation);
    await info.attach('usage-services-narrow',{body:await page.screenshot(),contentType:'image/png'});
    const urlBefore=page.url();await page.goBack();
    await expect.poll(()=>new URL(page.url()).searchParams.get('scope')).toBe(scope);
    await page.goForward();await expect(page).toHaveURL(urlBefore);
    await page.getByRole('tab',{name:words.usagePerf.viewNames.requests,exact:true}).click();
    const requests=page.getByRole('table',{name:words.usagePerf.requestsView,exact:true});
    await expect(requests.getByRole('row')).toHaveCount(3);
    await expect(requests).toContainText('studio-images');
    await expect(requests).toContainText(locale==='en'?'2 images':'2 张图像');
    await expect(requests).toContainText(locale==='en'?'– images':'– 张图像');
    await expect(requests).toContainText(words.usage.purposes.image_generation);
    await expect(requests).not.toContainText(words.usageRequest.uncached);
    expect(new URL(page.url()).searchParams.get('service')).toBe('studio-images');
    expect(new URL(page.url()).searchParams.get('purpose')).toBe('image_generation');
    expect(new URL(page.url()).searchParams.get('scope')).toBe(scope);
    await info.attach('usage-service-requests-narrow',{body:await page.screenshot(),contentType:'image/png'});
    await page.setViewportSize({width:1920,height:900});
    await expect(requests.getByRole('columnheader',{name:words.usagePerf.choices.splits.offering.name,exact:true})).toBeVisible();
    await info.attach('usage-service-requests-desktop',{body:await page.screenshot(),contentType:'image/png'});
  });
});
