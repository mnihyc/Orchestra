/** Shipped resource plans count immediate stops and keep migration separate from typed deletion. */
import {expect,test} from '@playwright/test';
import type {Agent,Provider,ResourceImpact} from '../../src/generated/api-types';
import {caption} from '../../e2e/capture/open';
import {screenReady} from '../../e2e/layout-rules-helpers';
import {resourceFixture,resourceLive,type Fixture} from './custody-resources';
import {replySent,scenario,signIn} from './support';

function impact(source:Fixture):ResourceImpact {
  const visible=[{object:{kind:'thread' as const,id:'019a0000-0000-7000-8000-000000000042'},name:'Repair redirects',relation:'binding' as const,until:null}];
  const empty={count:0,threads:0,visible:[],hidden:0,since:null};
  return {target:source.target,name:source.name,stopping:false,
    facts:{subject:{kind:'resource',object:source.target.object},as_of:'2026-10-28T00:00:00Z',dependents:[{relation:'binding',count:5,visible,hidden:4,until:null}],dependents_complete:true,uses:null},
    activity:{running:{count:2,threads:2,visible,hidden:1,since:null},waiting:empty,stopped:empty}};
}

for(const kind of ['provider','model','agent','environment'] as const)for(const act of ['disable','delete'] as const)for(const width of [1440,400])for(const theme of ['light','dark'] as const)for(const locale of ['en','zh-CN'] as const){
  // Environment deletion has its retained-files/mount fates in the existing removal matrix.
  if(kind==='environment'&&act==='delete')continue;
  test(`${act} ${kind} ${width}px ${theme} ${locale}`,async({page},info)=>{
    await scenario();await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'});
    const signed=page.waitForResponse((response)=>response.request().method()==='POST'&&new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);const auth=await signed;
    if(typeof auth!=='object'||auth===null||!('access_token'in auth)||typeof auth.access_token!=='string')throw new Error('The disposable sign-in has no access token');
    const headers={Authorization:`Bearer ${auth.access_token}`};expect((await page.request.patch('/api/v1/me',{headers,data:{theme,locale}})).ok()).toBe(true);
    const source=await resourceFixture(page,kind,headers,true);await resourceLive(page,source);let accepted=0;
    await page.route(/\/api\/v1\/resources\/impact$/u,(route)=>{expect(route.request().postDataJSON()).toEqual(source.target);return route.fulfill({status:200,json:impact(source)});});
    const path=act==='delete'?(kind==='model'?source.mutation:source.resourcePath):kind==='provider'||kind==='environment'?`${source.resourcePath}/disable`:source.mutation;
    const method=act==='delete'?'DELETE':source.method;
    await page.route(new RegExp(`${path}(?:\\?|$)`,'u'),async(route)=>{
      const request=route.request();if(request.method()!==method){await route.fallback();return;}
      if(act==='disable'){
        expect(new URL(request.url()).searchParams.get('note')).toBe('Key rotation');
        expect(request.postData()===null?null:request.postDataJSON()).toEqual(kind==='model'?{enabled:false}:kind==='agent'?{expected_revision:(source.read() as Agent).revision,disabled:true}:null);
      }
      accepted+=1;
      await route.fulfill(act==='delete'?{status:204}:{status:200,json:kind==='model'?(source.read() as Provider).models[0]:source.read()});
    });
    await page.goto(source.path);await screenReady(page,locale,theme);
    if(kind==='model'&&act==='disable')await page.getByRole('switch',{name:caption('@providers.models.enabledLabel',locale).replace('{{name}}',source.name),exact:true}).click();
    else if(act==='disable'&&kind==='provider')await page.getByRole('button',{name:caption('@providers.danger.disable',locale),exact:true}).click();
    else{
      await page.getByRole('button',{name:caption(kind==='model'?'@providers.models.menuOf':'@resources.act.menuOf',locale).replace('{{name}}',source.name),exact:true}).click();
      await page.getByRole('menuitem',{name:caption(act==='disable'?'@resources.act.disable':'@resources.act.delete',locale),exact:true}).click();
    }
    const prefix=kind==='agent'?'agents':kind==='environment'?'environments':'providers';
    const title=caption(`@${prefix}.danger.${act}Title`,locale).replace('{{name}}',source.name);
    const dialog=page.getByRole('alertdialog',{name:title,exact:true});await expect(dialog).toBeVisible();
    const confirm=dialog.getByRole('button',{name:caption(`@${prefix}.danger.${act}Confirm`,locale).replace('{{name}}',source.name),exact:true});
    expect(accepted).toBe(0);await expect(dialog.getByText(locale==='en'?'2 turns stop now':'立即停止 2 个轮次',{exact:true})).toBeVisible();
    await expect(dialog.getByRole('combobox')).toHaveCount(0);
    if(act==='delete'){
      await expect(dialog.getByRole('button',{name:caption('@resourceImpact.switchFirst',locale),exact:true})).toBeVisible();
      await expect(confirm).toBeDisabled();const typed=dialog.getByRole('textbox',{name:caption('@resourceImpact.typeName',locale).replace('{{name}}',source.name),exact:true});
      await typed.fill(`${source.name} extra`);await expect(confirm).toBeDisabled();expect(accepted).toBe(0);await typed.fill(source.name);
    }else{
      await expect(dialog.getByRole('button',{name:caption('@resourceImpact.switchFirst',locale),exact:true})).toHaveCount(0);
      await dialog.getByRole('textbox',{name:new RegExp(caption('@resourceImpact.note',locale),'u')}).fill('  Key rotation  ');
    }
    await expect(confirm).toBeEnabled();expect(await dialog.evaluate((element)=>element.scrollWidth<=element.clientWidth)).toBe(true);
    await page.screenshot({path:info.outputPath('resource-plan.png'),fullPage:true});
    await confirm.click();await expect(dialog).toHaveCount(0);expect(accepted).toBe(1);
  });
}
