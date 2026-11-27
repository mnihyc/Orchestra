/** Enable reads nothing first and keeps each resource's waiting messages an explicit choice. */
import { expect, test } from '@playwright/test';
import type { Provider } from '../../src/generated/api-types';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { replySent, scenario, signIn } from './support';
import {resourceFixture as fixture,resourceLive as live} from './custody-resources';

for(const kind of ['provider','model','agent','environment'] as const)for(const width of [1440,400])for(const theme of ['light','dark'] as const)for(const locale of ['en','zh-CN'] as const){
  test(`Enable ${kind} ${width}px ${theme} ${locale}`,async({page},info)=>{
    await scenario();await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    const signed=page.waitForResponse((response)=>response.request().method()==='POST'&&new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);const auth=await signed;
    if(typeof auth!=='object'||auth===null||!('access_token'in auth)||typeof auth.access_token!=='string')throw new Error('The disposable sign-in has no access token');
    const headers={Authorization:`Bearer ${auth.access_token}`};expect((await page.request.patch('/api/v1/me',{headers,data:{theme,locale}})).ok()).toBe(true);
    const source=await fixture(page,kind,headers);const publish=await live(page,source);let accepted=0;
    let impactReads=0;await page.route(/\/api\/v1\/resources\/impact$/u,(route)=>{impactReads+=1;return route.fallback();});
    const runWaiting=width===1440;
    await page.route(new RegExp(`${source.mutation}(?:\\?|$)`,'u'),async(route)=>{
      const request=route.request();if(request.method()!==source.method){await route.fallback();return;}
      expect(new URL(request.url()).searchParams.get('run_waiting')).toBe(String(runWaiting));
      expect(request.postData()===null?null:request.postDataJSON()).toEqual(source.body);
      accepted+=1;source.enable();publish();await route.fulfill({status:200,json:kind==='model'?(source.read() as Provider).models[0]:source.read()});
    });
    await page.goto(source.path);await screenReady(page,locale,theme);
    if(kind==='model')await page.getByRole('switch',{name:caption('@providers.models.enabledLabel',locale).replace('{{name}}',source.name),exact:true}).click();
    // The act that brings a disabled resource back stands among the head's buttons beside Edit, an environment's too; its menu keeps the rest.
    else await page.getByRole('button',{name:new RegExp(`^${caption('@resources.act.enable',locale)}`,'u')}).first().click();
    const dialog=page.getByRole('alertdialog',{name:caption('@resourceImpact.enableTitle',locale).replace('{{name}}',source.name),exact:true});
    const waiting=dialog.getByRole('checkbox',{name:caption('@resourceImpact.runWaiting',locale),exact:true});await expect(waiting).not.toBeChecked();expect(accepted).toBe(0);
    await expect(dialog.getByRole('button',{name:caption('@resources.act.enable',locale),exact:true})).toBeEnabled();if(runWaiting)await waiting.check();
    expect(await dialog.evaluate((element)=>element.scrollWidth<=element.clientWidth)).toBe(true);
    await page.screenshot({path:info.outputPath('enable-review.png'),fullPage:true});
    await dialog.getByRole('button',{name:caption('@resources.act.enable',locale),exact:true}).click();
    await expect(dialog).toHaveCount(0);expect(accepted).toBe(1);expect(impactReads).toBe(0);
  });
}
