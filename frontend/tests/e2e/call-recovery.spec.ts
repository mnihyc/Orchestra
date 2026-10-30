/** Recovery settings and live controls use the same bounded presentation in both languages, themes and widths. */
import { expect, test, type Page } from '@playwright/test';
import { readFile, writeFile } from 'node:fs/promises';
import { readArticle } from '../../src/help/guide.ts';
import type { CallRecovery } from '../../src/generated/api-types';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, configureThread, emit, scenario, signIn } from './support.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { USUAL_MODEL } from '../doubles/fixtures.ts';
import { agentMessage, errorItem, marker, threadInput, turn, userMessage } from '../doubles/thread-rows.ts';
import { chats } from './chat-support.ts';

async function contained(page: Page): Promise<void> {
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const dialogs = await page.getByRole('dialog').evaluateAll((elements) => elements.map((element) => {
    const box=element.getBoundingClientRect();
    return box.left>=-1 && box.right<=innerWidth+1 && box.top>=-1 && box.bottom<=innerHeight+1;
  }));
  expect(dialogs.every(Boolean)).toBe(true);
}

async function failedTurn(recovery:CallRecovery,ordinal:number):Promise<void> {
  const id=`recovery-turn-${ordinal}`,input=`recovery-input-${ordinal}`,at=new Date().toISOString();
  const failure=errorItem(`recovery-error-${ordinal}`,recovery.failure.message,{turn:id,at,retryable:true});
  if(failure.kind==='error') failure.payload={...failure.payload,code:recovery.failure.code};
  await emit(ACME.mainThread,{
    thread:{state:'blocked',current_turn_id:null,lock:null,blocked:{reason:'error',detail:'call_recovery',sandbox:null}},
    turns:[{...turn(id,{ordinal,state:'failed',inputId:input,model:USUAL_MODEL.name,display:USUAL_MODEL.display,effort:USUAL_MODEL.effort,startedAt:at,completedAt:at}),error:recovery.failure}],
    inputs:[{...threadInput(input,'Complete the task',{author:ADA,state:'delivered',position:String(ordinal)}),turn_id:id,delivered_at:at}],
    items:[userMessage(`recovery-message-${ordinal}`,'Complete the task',{author:ADA,turn:id,inputId:input,at}),...(recovery.failure.incomplete?[agentMessage(`recovery-partial-${ordinal}`,'One change is already made',{turn:id,at})]:[]),failure,{...failure,id:`recovery-error-${ordinal}-repeat`}],
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440,400]) {
  test(`call recovery ${locale} ${theme} ${width}`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await page.setViewportSize({width,height:900});
    await chats('/locale',{as:'ada',locale});
    await signIn(page);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const catalogue=locale==='en'?en:zh;
    const words=catalogue.calls;
    const common=catalogue.common;
    const counts={settingsWrites:0,recoveryReads:0,recoveryActions:[] as string[]};
    page.on('request',(request)=>{
      if(new URL(request.url()).pathname.endsWith('/settings/calls.rules') && request.method()==='PUT') counts.settingsWrites++;
    });
    await page.goto(`/project/${ACME.web}/settings?section=policies&setting=calls.rules`);
    const setting=page.locator('[data-setting="calls.rules"]');
    await expect(setting).toBeVisible();
    await setting.locator('[data-setting-editor-trigger]').click();
    await page.getByRole('button',{name:/→/}).first().click();
    const editor=page.getByRole('dialog',{name:words.editRule,exact:true});
    await editor.getByLabel(words.delay,{exact:true}).fill('75');
    await editor.getByRole('region',{name:words.editRule,exact:true}).evaluate((element)=>{element.scrollTop=0;});
    await expect(editor.getByText(words.reason,{exact:true})).toBeInViewport();
    await expect(editor.getByText(words.provider,{exact:true})).toBeInViewport();
    for(const placeholder of [words.anyModel,words.anyStatus,words.anyWords,words.anyPattern]) await expect(editor.getByPlaceholder(placeholder,{exact:true})).toHaveValue('');
    await expect(editor.getByRole('columnheader',{name:words.steps,exact:true})).toBeVisible();
    await contained(page);
    await info.attach('rule-editor',{body:await page.screenshot({path:info.outputPath('rule-editor.png')}),contentType:'image/png'});
    await editor.getByRole('columnheader',{name:words.steps,exact:true}).evaluate((element)=>element.scrollIntoView({block:'start'}));
    await expect(editor.getByRole('columnheader',{name:words.steps,exact:true})).toBeInViewport();
    await contained(page);
    await info.attach('rule-editor-step',{body:await page.screenshot({path:info.outputPath('rule-editor-step.png')}),contentType:'image/png'});
    const helpArticle=readArticle('model-call-recovery',await readFile(new URL(locale==='en'?'../../../docs/user/model-call-recovery.md':'../../../docs/user/zh-CN/model-call-recovery.md',import.meta.url),'utf8'));
    const ruleHeading=helpArticle.headings.find((heading)=>heading.text===(locale==='en'?'Set rules':'设置规则'))?.id;
    if(ruleHeading===undefined)throw new Error('the authored recovery rules heading must ship with the screen');
    await editor.locator(`[data-help-article="model-call-recovery"][data-help-heading="${ruleHeading}"]`).click();
    const ruleHelp=page.getByRole('dialog',{name:words.help.rules.title,exact:true});
    await expect(ruleHelp).toBeVisible();
    await contained(page);
    await ruleHelp.getByRole('button',{name:catalogue.help.hint.more,exact:true}).click();
    const reading=page.locator('[data-help-reading]');
    await expect(reading).toBeVisible();
    await expect(reading.locator(`[id="${ruleHeading}"]`)).toBeInViewport();
    await info.attach('recovery-help',{body:await page.screenshot({path:info.outputPath('recovery-help.png')}),contentType:'image/png'});
    await reading.getByRole('button',{name:catalogue.helpFlow.back,exact:true}).click();
    await expect(editor.getByLabel(words.delay,{exact:true})).toHaveValue('75');
    await editor.getByRole('button',{name:common.save,exact:true}).click();
    await expect.poll(()=>counts.settingsWrites).toBe(1);
    await page.keyboard.press('Escape');

    let recovery:CallRecovery={state:'scheduled',failure:{code:'provider_busy',message:'Synthetic model capacity',retryable:true,window:null,said:null,resets_at:null,incomplete:false},rule_id:'busy',step:{type:'retry',delay_secs:60,times:3,backoff:2},attempt:2,due_at:new Date(Date.now()+60_000).toISOString(),detail:null,count:null,window_secs:null};
    await page.route(`**/api/v1/threads/${ACME.mainThread}/calls/recovery`,async(route)=>{
      if(route.request().method()==='POST') {
        const body=route.request().postDataJSON() as {action:string};counts.recoveryActions.push(body.action);
        recovery={...recovery,state:'stopped',step:null,due_at:null,detail:'repeat_limit',count:4,window_secs:300};
      } else counts.recoveryReads++;
      await route.fulfill({json:{recovery}});
    });
    await configureThread(ACME.mainThread,{reset:true});
    await failedTurn(recovery,1);
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.getByRole('button',{name:words.retryNow,exact:true})).toBeVisible();
    await expect(page.getByRole('button',{name:catalogue.timeline.footer.retry,exact:true})).toHaveCount(0);
    await expect(page.locator('[data-item="recovery-error-1"]')).toHaveCount(0);
    await expect(page.locator('[data-turn-failure]')).toHaveCount(1);
    await expect(page.locator('[data-item="recovery-error-1-repeat"]')).toHaveCount(0);
    await expect(page.locator('[data-turn-failure]')).toContainText(words.failureAttempts_other.replace('{{attempts}}','2'));
    await expect(page.getByRole('checkbox',{name:words.stop,exact:true})).toHaveCount(0);
    await contained(page);
    await info.attach('scheduled',{body:await page.screenshot({path:info.outputPath('scheduled.png')}),contentType:'image/png'});
    await page.getByRole('button',{name:words.stop,exact:true}).click();
    await expect(page.getByRole('button',{name:words.retry,exact:true})).toBeVisible();
    await expect(page.getByRole('button',{name:words.continue,exact:true})).toBeVisible();
    const switching=page.getByRole('button',{name:words.switch,exact:true});
    await expect(switching).toBeVisible();
    if(await switching.getAttribute('aria-disabled')==='true'){await switching.focus();await expect(page.getByRole('tooltip')).toContainText(words.configureRequired);await page.keyboard.press('Escape');}
    await expect(page.getByRole('status',{name:words.status,exact:true})).toHaveCount(0);
    await expect(page.locator('[data-call-recovery="stopped"]')).toHaveCount(1);
    await expect(page.locator('[data-call-recovery="stopped"]')).toContainText(locale==='en'?'5 min':'5分钟');
    await expect(page.getByRole('button',{name:catalogue.timeline.footer.retry,exact:true})).toHaveCount(0);
    await contained(page);
    await info.attach('stopped',{body:await page.screenshot({path:info.outputPath('stopped.png')}),contentType:'image/png'});
    expect(counts.recoveryActions).toEqual(['stop']);
    // The banner and live line share a query; initial metadata hydration may cause one sequence refresh.
    expect(counts.recoveryReads).toBeGreaterThan(0);
    expect(counts.recoveryReads).toBeLessThanOrEqual(2);
    expect(counts.settingsWrites).toBe(1);
    const beforeReopen=counts.recoveryReads;
    await page.goto(`/project/${ACME.web}/settings?section=policies&setting=calls.rules`);
    recovery={...recovery,state:'scheduled',rule_id:'refused',attempt:1,continuing:true,failure:{...recovery.failure,code:'model_refused',reason:'cyber',message:'Synthetic vendor filter refusal',incomplete:true},step:{type:'continue',times:3},due_at:new Date(Date.now()+30_000).toISOString(),detail:null,count:null,window_secs:null};
    await failedTurn(recovery,2);
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-call-recovery="scheduled"]').first()).toContainText(words.kinds.model_refused);
    await contained(page);
    expect(counts.recoveryReads).toBeGreaterThan(beforeReopen);
    expect(counts.recoveryReads-beforeReopen).toBeLessThanOrEqual(2);
    await expect(page.getByText(catalogue.timeline.failure.incomplete,{exact:true})).toHaveCount(0);
    await expect(page.getByRole('button',{name:catalogue.timeline.footer.retry,exact:true})).toHaveCount(0);
    await info.attach('continuing',{body:await page.screenshot({path:info.outputPath('continuing.png')}),contentType:'image/png'});

    const input='recovery-automatic-input',id='recovery-automatic-turn',at=new Date().toISOString();
    const message=userMessage('recovery-automatic-message','Continue',{author:ADA,turn:id,inputId:input,at});
    if(message.kind!=='user_message') throw new Error('the continuation must be a user-message row');
    message.payload={...message.payload,automatic:{rule_id:'refused',kind:'model_refused',attempt:1}};
    recovery={...recovery,state:'running',due_at:null};
    await emit(ACME.mainThread,{
      thread:{state:'running',current_turn_id:id,blocked:null},
      turns:[turn(id,{ordinal:3,state:'running',inputId:input,model:USUAL_MODEL.name,display:USUAL_MODEL.display,effort:USUAL_MODEL.effort,startedAt:at})],
      inputs:[{...threadInput(input,'Continue',{author:ADA,state:'delivered',position:'3',clientId:'recovery:recovery-turn-2'}),turn_id:id,delivered_at:at}],
      items:[marker('recovery-automatic-step',{marker:'call_recovery',data:{action:'continue',rule:'refused',kind:'model_refused',attempt:1,total:3,automatic:true,input_id:input}},{at}),message,agentMessage('recovery-resumed-work','The next change is under way',{turn:id,at,status:'in_progress'})],
    });
    const automatic=words.automatic;
    const annotation=page.getByText(automatic,{exact:false});
    await expect(annotation).toBeVisible();
    await expect(page.getByText(catalogue.timeline.failure.incomplete,{exact:true})).toHaveCount(0);
    await expect(page.getByRole('button',{name:catalogue.timeline.footer.retry,exact:true})).toHaveCount(0);
    const event=page.locator('[data-recovery-event]');
    await expect(event).toHaveCount(1);
    await expect(event).toContainText(words.causes.model_refused);
    await expect(event).toContainText(words.attempt.replace('{{attempt}}','1').replace('{{total}}','3'));
    await expect(event.getByRole('link')).toHaveAttribute('href',/setting=calls\.rules/);
    await expect(event.locator('time')).toHaveCount(1);
    await expect(page.locator('[data-item="recovery-automatic-message"]')).toContainText('Ada');
    const latest=page.getByRole('button',{name:catalogue.timeline.jump,exact:true});
    if(await latest.isVisible()) await latest.click();
    else await annotation.scrollIntoViewIfNeeded();
    await expect(annotation).toBeInViewport();
    await contained(page);
    await info.attach('automatic-continuation',{body:await page.screenshot({path:info.outputPath('automatic-continuation.png')}),contentType:'image/png'});
    await annotation.focus();
    await expect(page.getByRole('tooltip')).toContainText(words.automaticDetail.replace('{{rule}}','refused').replace('{{attempt}}','1'));
    await contained(page);
    await info.attach('automatic-tooltip',{body:await page.screenshot({path:info.outputPath('automatic-tooltip.png')}),contentType:'image/png'});
    await page.keyboard.press('Escape');
    await annotation.evaluate((element)=>{if(element instanceof HTMLElement)element.blur();});
    const automaticHeading=helpArticle.headings.find((heading)=>heading.text===(locale==='en'?'Automatic work and access':'自动工作与权限'))?.id;
    if(automaticHeading===undefined)throw new Error('the authored automatic-work heading must ship with the screen');
    await page.locator(`[data-help-article="model-call-recovery"][data-help-heading="${automaticHeading}"]`).click();
    const automaticHelp=page.getByRole('dialog',{name:words.help.automatic.title,exact:true});
    await expect(automaticHelp.getByText(words.help.automatic.what,{exact:true})).toBeVisible();
    await contained(page);
    await info.attach('automatic-help',{body:await page.screenshot({path:info.outputPath('automatic-help.png')}),contentType:'image/png'});
    await page.keyboard.press('Escape');
    await expect(automaticHelp).toHaveCount(0);
    const metrics=info.outputPath('request-counts.json');
    await writeFile(metrics,JSON.stringify(counts,null,2));
    await info.attach('request-counts',{path:metrics,contentType:'application/json'});
  });
}
