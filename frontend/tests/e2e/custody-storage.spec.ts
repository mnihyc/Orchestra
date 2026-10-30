/** The shipped Storage report holds at desktop and phone widths in both languages and themes. */
import { expect,test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { readStorageSearch,storageRequest } from '../../src/features/storage/search';
import { storageReport } from '../doubles/storage-report';
import { replySent,scenario,signIn } from './support';

for(const width of [1440,400]) for(const theme of ['light','dark'] as const) for(const locale of ['en','zh-CN'] as const) {
  test(`storage report ${width}px ${theme} ${locale}`,async({page},info)=>{
    await scenario();
    await page.setViewportSize({width,height:900});
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    await page.route('**/api/v1/storage*',async(route)=>{
      const query=storageRequest(readStorageSearch(Object.fromEntries(new URL(route.request().url()).searchParams)));
      await route.fulfill({status:200,json:storageReport(query)});
    });
    const signedIn=page.waitForResponse((response)=>response.request().method()==='POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth=await signedIn;
    if(typeof auth!=='object' || auth===null || !('access_token' in auth) || typeof auth.access_token!=='string') throw new Error('The disposable sign-in has no access token.');
    expect((await page.request.patch('/api/v1/me',{headers:{Authorization:`Bearer ${auth.access_token}`},data:{theme,locale}})).ok()).toBe(true);
    await page.goto('/storage');
    await screenReady(page,locale,theme);
    await expect(page.getByText('Workspace recovery',{exact:true})).toBeVisible();
    await expect(page.getByRole('combobox',{name:caption('@storage.kind',locale),exact:true})).toContainText(caption('@storage.allKinds',locale));
    await expect(page.getByRole('combobox',{name:caption('@storage.why',locale),exact:true})).toContainText(caption('@storage.allReasons',locale));
    await expect(page.getByRole('list',{name:caption('@storage.figures',locale)})).toContainText('≥4 KiB');
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
    await page.screenshot({path:info.outputPath('storage-report.png'),fullPage:true});
    await page.getByText('Workspace recovery',{exact:true}).scrollIntoViewIfNeeded();
    const records=page.getByRole('table',{name:caption('@storage.records',locale),exact:true});
    expect(await records.evaluate((table)=>table.scrollWidth<=table.clientWidth)).toBe(true);
    await page.screenshot({path:info.outputPath('storage-records.png'),fullPage:true});
    await page.getByRole('combobox',{name:caption('@storage.kind',locale),exact:true}).click();
    await page.getByRole('option',{name:caption('@storage.kinds.image',locale),exact:true}).click();
    await expect(page).toHaveURL(/kind=image/u);
    await expect(page.getByText('Workspace recovery',{exact:true})).toHaveCount(0);
    await expect(page.getByText('Cached image',{exact:true})).toBeVisible();
    await page.goto('/storage');
    await expect(page).toHaveURL(/kind=image/u);
    await expect(page.getByText('Cached image',{exact:true})).toBeVisible();
    await page.getByRole('button',{name:caption('@storage.clearFilters',locale),exact:true}).click();
    await expect(page.getByText('Workspace recovery',{exact:true})).toBeVisible();
    await page.getByRole('button',{name:caption('@storage.guide',locale),exact:true}).click();
    await expect(page.getByRole('heading',{name:locale==='en'?'Storage report':'存储报告',exact:true})).toBeVisible();
    await page.screenshot({path:info.outputPath('storage-help.png'),fullPage:true});
  });
}
