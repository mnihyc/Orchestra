/** Resource library help retains its target, text alignment, keyboard behavior and read boundary. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import { scenario, signIn, replySent } from './support';
import { asAdmin } from './admin-mode-support';
import { RUNS_ON } from '../doubles/fixtures';
import { resourceFixtures } from '../../e2e/layout-fixtures';
import { inspectLayout } from '../../e2e/layout-rules-dom';
import { LAYOUT_INPUT, screenReady } from '../../e2e/layout-rules-helpers';

for(const width of [1440,400]) for(const theme of ['light','dark'] as const) for(const locale of ['en','zh-CN'] as const) for(const [family,route] of [
 ['detail',`/resources/environments/${RUNS_ON.workspace}?tab=limits`],['list','/resources/providers'],
] as const) test(`resource library Help keeps 24px and aligns its own line ${family} ${locale} ${theme} ${width}px`,async({page},info)=>{
 await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme:theme});await scenario();await resourceFixtures(page);
 const loggedIn=page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname.endsWith('/auth/login')).then(replySent);
 await signIn(page);const auth=await loggedIn;
 if(typeof auth!=='object'||auth===null||!('access_token'in auth)||typeof auth.access_token!=='string')throw new Error('Native login token missing');
 await asAdmin(page,true);
 expect((await page.request.patch('/api/v1/me',{headers:{Authorization:`Bearer ${auth.access_token}`},data:{theme,locale}})).ok()).toBe(true);
 const requests:string[]=[];page.on('request',r=>{const u=new URL(r.url());if(u.pathname.startsWith('/api/v1/')&&u.pathname!=='/api/v1/ws')requests.push(`${r.method()} ${u.pathname}${u.search}`);});
 await page.goto(route);await screenReady(page,locale,theme);await page.evaluate(async()=>{await document.fonts.ready;});
 const baseline=await page.evaluate(inspectLayout,LAYOUT_INPUT);
 const label=locale==='en'?'Help: Resource library':'帮助：资源库';
 const target=page.getByRole('button',{name:label,exact:true});await expect(target).toBeVisible();
 const geometry=await target.evaluate((button)=>{
  const describe=(element:Element|null)=>{
   if(element===null)return null;
   const box=element.getBoundingClientRect();
   return {box:{width:box.width,height:box.height,centre:box.y+box.height/2}};
  };
  return {button:describe(button),svg:describe(button.querySelector('svg'))};
 });
 await info.attach('baseline-geometry',{body:JSON.stringify({route,locale,theme,width,family,geometry,allFaults:baseline.faults},null,2),contentType:'application/json'});
 await info.attach('baseline-screen',{body:await page.screenshot({fullPage:true,animations:'disabled'}),contentType:'image/png'});
 expect(geometry.button?.box.width).toBeGreaterThanOrEqual(24);expect(geometry.button?.box.height).toBeGreaterThanOrEqual(24);
 expect(Math.abs((geometry.svg?.box.centre??0)-(geometry.button?.box.centre??0))).toBeLessThan(1);
 await info.attach('initial-requests',{body:JSON.stringify({count:requests.length,requests}),contentType:'application/json'});
 expect(baseline.faults.filter(f=>f.rule==='icon-centre'&&f.text===label)).toEqual([]);
 const initialAX=await new AxeBuilder({page}).analyze();
 expect(initialAX.violations.filter(v=>v.impact==='serious'||v.impact==='critical')).toEqual([]);
 const offsetBefore=await target.getAttribute('style');
 try{
  await target.evaluate(button=>{button.style.transform='translateY(1px)';});
  const displaced=await page.evaluate(inspectLayout,LAYOUT_INPUT);
  expect(displaced.faults.some(f=>f.rule==='icon-centre'&&f.text===label),'A onepixel displacement must remain a finding').toBe(true);
 }finally{await target.evaluate((button,style)=>{if(style===null)button.removeAttribute('style');else button.setAttribute('style',style);},offsetBefore);}
 const count=requests.length;await target.focus();await page.keyboard.press('Enter');
 await expect(page.getByRole('dialog')).toBeVisible();await page.waitForLoadState('networkidle');
 const openAX=await new AxeBuilder({page}).analyze();
 expect(openAX.violations.filter(v=>v.impact==='serious'||v.impact==='critical')).toEqual([]);
 expect(requests.slice(count),'Opening existing help must add no API reads').toEqual([]);
 await page.keyboard.press('Escape');await expect(page.getByRole('dialog')).toHaveCount(0);await expect(target).toBeFocused();
 await info.attach('complete-requests',{body:JSON.stringify({count:requests.length,requests}),contentType:'application/json'});
});
