/** A level chosen at a place is found and reset from personal settings, without follow-up reads. */
import { expect,test,type Request } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { bodySent, ACME,scenario,signIn } from './support';

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)test(`notification exceptions ${locale}/${theme}`,async({page},info)=>{
  await scenario();await page.emulateMedia({colorScheme:theme});await signIn(page);
  // Seed the preference through its ordinary bell route; the browser workflow below performs
  // the personal screen's single mutation and inspects what a person sees from that answer.
  await page.goto(`/thread/${ACME.mainThread}`);
  const words=locale==='en'?english:chinese;
  if(locale==='zh-CN'){
    await page.goto('/settings/interface');await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();
    await page.getByRole('option',{name:'简体中文',exact:true}).click();
  }
  await page.goto(`/thread/${ACME.mainThread}`);
  const bell=page.getByRole('button',{name:words.thread.bell.label,exact:true});
  await bell.click();
  await page.getByRole('menuitemradio',{name:words.choiceCatalog.subscriptionLevel.important.name,exact:true}).click();
  await expect(bell).toContainText(words.notificationBell.level.important);
  await expect(bell).not.toContainText(words.notificationBell.level.off);
  await bell.click();
  const checked=page.getByRole('menuitemradio',{name:words.choiceCatalog.subscriptionLevel.important.name,exact:true});
  await expect(checked).toHaveAttribute('aria-checked','true');
  await expect(checked.locator('svg.lucide-check')).toBeVisible();
  const defaultName=words.notificationBell.defaultWithLevel.replace('{{level}}',words.notificationBell.level.off);
  const defaultOption=page.getByRole('menuitemradio',{name:defaultName,exact:true});
  await expect(defaultOption).toHaveAccessibleName(defaultName);
  await expect(page.getByRole('menu').locator('[data-choice-description]')).toHaveCount(0);
  const bellReads:string[]=[];
  const observeBellRead=(request:Request)=>{
    const path=new URL(request.url()).pathname;
    if(request.method()==='GET' && (path.endsWith(`/nodes/${ACME.mainThread}`) || path.endsWith('/me/notification-exceptions')))bellReads.push(path);
  };
  page.on('request',observeBellRead);
  const offWrite=page.waitForRequest((request)=>request.method()==='PUT' && request.url().includes(`/me/notification-exceptions/${ACME.mainThread}`));
  await page.getByRole('menuitemradio',{name:words.choiceCatalog.subscriptionLevel.off.name,exact:true}).click();
  expect((await bodySent(await offWrite))).toEqual({level:'off'});
  await expect(bell).toContainText(words.notificationBell.level.off);
  await expect(bell.locator('svg.lucide-bell-off')).toBeVisible();
  await bell.focus();await expect(bell).toBeFocused();
  await expect(bell).toHaveAccessibleDescription(words.notificationBell.level.off);
  page.off('request',observeBellRead);
  expect(bellReads).toEqual([]);
  // Restore a real differing level before reviewing its row from personal settings.
  await bell.click();await page.getByRole('menuitemradio',{name:words.choiceCatalog.subscriptionLevel.important.name,exact:true}).click();
  await expect(bell).toHaveAccessibleDescription(words.notificationBell.level.important);
  for(const width of [1440,400]){
    await page.setViewportSize({width,height:1000});
    await expect(bell).toBeVisible();await expect(bell).toContainText(words.notificationBell.level.important);
    expect(await bell.evaluate((element)=>element.getBoundingClientRect().height)).toBe(28);
    await info.attach(`thread-bell-${width}-${locale}-${theme}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
  }
  await page.goto('/settings/notifications');
  const list=page.getByRole('list',{name:words.notificationExceptions.title});await expect(list).toBeVisible();
  await expect(page.getByRole('switch',{name:words.notifications.approvals.always,exact:true})).toBeChecked();
  await expect(page.getByRole('switch',{name:words.notifications.quiet.switch,exact:true})).not.toBeChecked();
  await expect(page.getByText(words.notifications.approvals.always,{exact:true})).toHaveCount(1);
  await expect(page.getByText(words.notifications.approvals.hint,{exact:true})).toBeVisible();
  const actualRow=list.getByRole('listitem').filter({has:page.locator(`a[href*="${ACME.mainThread}"]`)});
  await expect(actualRow).toBeVisible();
  for(const width of [1440,400]){
    await page.setViewportSize({width,height:1000});await expect(actualRow.getByRole('combobox')).toBeVisible();
    await info.attach(`notification-exceptions-${width}-${locale}-${theme}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
  }
  const reads:string[]=[];const writes:string[]=[];
  page.on('request',(request)=>{const path=new URL(request.url()).pathname;
    if(path.endsWith('/me/notification-exceptions') && request.method()==='GET')reads.push(path);
    if(path.includes('/me/notification-exceptions/') && request.method()==='PUT')writes.push(path);
  });
  await actualRow.getByRole('combobox').click();
  const defaultPrefix=words.notificationExceptions.default.split(' · ')[0] ?? words.notificationExceptions.default;
  await page.getByRole('option',{name:new RegExp('^'+defaultPrefix)}).click();
  await expect(page.locator(`a[href*="${ACME.mainThread}"]`).and(page.getByRole('list',{name:words.notificationExceptions.title}).getByRole('link'))).toHaveCount(0);
  expect(writes).toHaveLength(1);expect(reads).toEqual([]);
});

test('a project default and a thread exception can both be changed on the screens, and Default names the ancestor',async({page})=>{
  await scenario();await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await page.getByRole('button',{name:'Notifications for this place',exact:true}).click();
  await page.getByRole('menuitemradio',{name:'All',exact:true}).click();
  await expect(page.getByRole('button',{name:'Notifications for this place',exact:true})).toHaveAccessibleDescription('All');
  await page.goto(`/thread/${ACME.mainThread}`);
  const bell=page.getByRole('button',{name:english.thread.bell.label,exact:true});
  await expect(bell).toHaveAccessibleDescription('Default: All');
  await bell.click();await page.getByRole('menuitemradio',{name:'Off',exact:true}).click();
  await expect(bell).toContainText('Off');
  await bell.click();
  await expect(page.getByRole('menuitemradio',{name:'Default (All)',exact:true})).toBeVisible();
  await page.keyboard.press('Escape');
  await page.goto('/settings/notifications');
  const list=page.getByRole('list',{name:'Exceptions',exact:true});
  const thread=list.getByRole('listitem').filter({has:page.locator(`a[href*="${ACME.mainThread}"]`)});
  const project=list.getByRole('listitem').filter({has:page.locator(`a[href*="${ACME.web}"]`)});
  await expect(project).toBeVisible();await expect(thread).toBeVisible();
  await thread.getByRole('combobox').click();await page.getByRole('option',{name:'Default · All',exact:true}).click();
  await expect(thread).toHaveCount(0);await expect(project).toBeVisible();
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(bell).toHaveAccessibleDescription('Default: All');
  await expect(bell.locator('svg.lucide-bell-ring')).toBeVisible();
});
