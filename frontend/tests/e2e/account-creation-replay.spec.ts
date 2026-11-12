/** A lost public creation can be resent; its completed receipt leads to sign-in without another session. */
import { expect, test } from '@playwright/test';
import { scenario } from './support.ts';

const WORDS = {
  en: { username:'Username',name:'Name',password:'Password',setup:'Create administrator',register:'Create account',
    already:'Account already created',signIn:'Sign in' },
  'zh-CN': { username:'用户名',name:'姓名',password:'密码',setup:'创建管理员',register:'创建账号',
    already:'账号已创建',signIn:'登录' },
} as const;
for (const locale of ['en','zh-CN'] as const) for (const width of [400,1440])
for (const theme of ['light','dark'] as const) for (const kind of ['setup','register'] as const)
for (const outcome of ['receipt','refused'] as const) {
  test(`${kind} resend ${outcome} at ${width}px in ${locale}/${theme}`,async({page})=>{
    await scenario({scenario:kind==='setup'?'fresh':'team',meta:{registration_mode:'open'}});
    await page.addInitScript(({locale,theme})=>{
      localStorage.setItem('orch.appearance',JSON.stringify({locale,theme,density:'comfortable'}));
    },{locale,theme});
    await page.setViewportSize({width,height:900});
    await page.emulateMedia({colorScheme:theme});
    const attempts:{key:string|undefined;body:string|null}[]=[];
    let completed=false;
    // The surface control supplies the native protocol's two outcomes; native
    // account/session cardinality and password proof are decided by Rust cases.
    await page.route(`**/api/v1/${kind==='setup'?'setup':'auth/register'}`,async(route)=>{
      const request=route.request();
      attempts.push({key:request.headers()['idempotency-key'],body:request.postData()});
      if(attempts.length===1) {await route.abort('connectionfailed');return;}
      completed=outcome==='receipt';
      await route.fulfill({status:completed?(kind==='setup'?200:201):409,contentType:'application/json',
        body:JSON.stringify(completed?{projection:'receipt',user_id:'00000000-0000-4000-8000-000000000001',sign_in_required:true}:
          {type:'orch:error:conflict.exists',code:'conflict.exists',status:409,title:'Request refused',
            detail:'The account change could not be completed',request_id:'refused-registration'})});
    });
    if(kind==='setup') await page.route('**/api/v1/meta',async(route)=>{
      const response=await route.fetch();
      const body:unknown=await response.json();
      if(typeof body!=='object'||body===null||Array.isArray(body)) throw new Error('Meta did not return an object');
      await route.fulfill({response,json:{...body,needs_setup:!completed}});
    });
    const sessionWrites:string[]=[];
    page.on('request',(request)=>{
      if(attempts.length>0 && request.method()==='POST' && /\/auth\/(login|refresh)$/.test(new URL(request.url()).pathname)) sessionWrites.push(request.url());
    });
    await page.goto(kind==='setup'?'/setup':'/register');
    const w=WORDS[locale];
    await page.getByLabel(w.username,{exact:true}).fill('new-account');
    await page.getByLabel(w.name,{exact:true}).fill('New account');
    await page.getByLabel(w.password,{exact:true}).fill('a-valid-secret-1234');
    const submit=page.getByRole('button',{name:w[kind],exact:true});
    await submit.click();
    await expect.poll(()=>attempts.length).toBe(1);
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(submit).toBeEnabled();
    await submit.focus();await page.keyboard.press('Enter');
    await expect.poll(()=>attempts.length).toBe(2);
    expect(attempts[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
    expect(attempts[1]).toEqual(attempts[0]);
    if(outcome==='receipt') {
      await expect(page.getByText(w.already,{exact:true})).toBeVisible();
      await expect(submit).toHaveCount(0);
      const signIn=page.getByRole('link',{name:w.signIn,exact:true});
      await expect(signIn).toHaveAttribute('href','/login');
      expect(sessionWrites).toEqual([]);
      await signIn.focus();await page.keyboard.press('Enter');
      await expect(page).toHaveURL(/\/login$/);
    } else {
      await expect(page.getByText(w.already,{exact:true})).toHaveCount(0);
      await expect(page.getByLabel(w.username,{exact:true})).toBeVisible();
      await expect(submit).toBeEnabled();
      expect(sessionWrites).toEqual([]);
    }
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  });
}
