/** The joined frontend keeps managed settings, effective approval and quota words in one history. */
import { expect, test } from '@playwright/test';
import { marker } from '../doubles/thread-rows.ts';
import { ACME, DOUBLE, emit, scenario, signIn } from './support.ts';

const words={
  en:{approval:'Claude Code reported its approval mode as Bypass program prompts.',pins:'Claude Code reported its managed launch settings.',rate:'rate limit: Native quota warning'},
  'zh-CN':{approval:'Claude Code 报告的审批模式为跳过程序审批提示。',pins:'Claude Code 报告了受管启动设置。',rate:'速率限制：Native quota warning'},
} as const;
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const) test(`native observations survive the thread tab click path (${locale}, ${theme})`,async({page},info)=>{
  await scenario();
  await signIn(page);
  const appearance=await fetch(`${DOUBLE}/__double/first-steps/appearance`,{method:'POST',headers:{'Content-Type':'application/json'},
    body:JSON.stringify({locale,theme}),signal:AbortSignal.timeout(10_000)});
  expect(appearance.ok).toBe(true);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-thread-header]')).toBeVisible();
  const base={program:'Claude Code',subject:null,previous:{state:'unsupported' as const},expected:{state:'unsupported' as const},
    changed:false,diverged:false,next_turn:false,turn_id:null,reported_at:'2026-11-08T03:00:00Z'};
  await emit(ACME.mainThread,{items:[
    marker('native-effective-approval',{marker:'harness_changed',data:{...base,change:'approval_mode',value:{native_mode:'bypassPermissions'},
      expected:{state:'told',value:{native_mode:'default'}},diverged:true}}),
    marker('native-managed-readback',{marker:'harness_changed',data:{...base,change:'managed_settings_readback',value:{status:'reported'}}}),
    marker('native-quota-notice',{marker:'harness_changed',data:{...base,change:'rate_limit',value:{status:'allowed_warning',message:'Native quota warning'}}}),
  ]});
  await page.goto('/org/acme');
  await page.getByRole('tab',{name:/Main thread/}).first().click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ACME.mainThread}$`));
  for(const width of [1440,400]){
    await page.setViewportSize({width,height:900});
    await expect(page.getByText(words[locale].approval,{exact:true})).toBeVisible();
    await expect(page.getByText(words[locale].pins,{exact:true})).toBeVisible();
    await expect(page.getByText(words[locale].rate,{exact:false})).toBeVisible();
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
    await page.screenshot({path:info.outputPath(`native-observations-${width}.png`)});
  }
});
