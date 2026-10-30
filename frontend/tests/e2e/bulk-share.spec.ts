/** A real ticked list opens the common Share form; one failed page does not stop the permitted pages. */
import { expect, test } from '@playwright/test';

import { dialog, second, sent, stage } from './give-support.ts';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('two selected topics share together and Tell them sends one message containing both links', async ({page,browser,baseURL}) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}?view=list`);
  await page.getByRole('checkbox',{name:'Select Fix login',exact:true}).check();
  await page.getByRole('checkbox',{name:'Select Upgrade deps',exact:true}).check();
  await page.getByRole('region',{name:'Selected topics'}).getByRole('button',{name:'Share…',exact:true}).click();
  await expect(dialog(page)).toHaveAccessibleName('Access to 2 topics');
  await stage(page,'Vera Rubin');
  await dialog(page).getByRole('textbox',{name:'A line to go with it'}).fill('Please read both selected topics');
  await dialog(page).getByRole('button',{name:'Review shares',exact:true}).click();
  await expect(dialog(page).getByRole('region',{name:'Sharing consequences'})).toHaveCount(2);
  expect(await sent()).toEqual([]);
  await dialog(page).getByRole('button',{name:'Apply to 2 ready rows',exact:true}).click();
  await expect(dialog(page).getByText('Succeeded',{exact:true})).toHaveCount(2);
  const vera = await second(browser,baseURL,'/org/acme/messages',PEOPLE.vera);
  await vera.page.getByText('Ada Lovelace').first().click();
  const message = vera.page.locator('li[data-message]',{hasText:'Please read both selected topics'});
  await expect(message).toHaveCount(1);
  await expect(message.getByRole('link',{name:'Fix login',exact:true})).toHaveAttribute('href',`/topic/${ACME.fixLogin}`);
  await expect(message.getByRole('link',{name:'Upgrade deps',exact:true})).toHaveAttribute('href',`/topic/${ACME.upgradeDeps}`);
  await vera.context.close();
});

test('a page which can no longer be read is reported individually while the other selected topic is shared', async ({page}) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}?view=list`);
  await page.getByRole('checkbox',{name:'Select Fix login',exact:true}).check();
  await page.getByRole('checkbox',{name:'Select Upgrade deps',exact:true}).check();
  await page.route(`**/nodes/${ACME.upgradeDeps}/permissions`,async (route) => {
    if (route.request().method() !== 'GET') { await route.continue(); return; }
    await route.fulfill({status:404,contentType:'application/problem+json',body:JSON.stringify({type:'orch:error:not_found',code:'not_found',status:404,title:'Not found',detail:'This selected page can no longer be read',request_id:'bulk-lost-page'})});
  });
  await page.getByRole('region',{name:'Selected topics'}).getByRole('button',{name:'Share…',exact:true}).click();
  await stage(page,'Vera Rubin');
  await dialog(page).getByRole('checkbox',{name:/Send Vera Rubin a message with the link/}).uncheck();
  await dialog(page).getByRole('button',{name:'Review shares',exact:true}).click();
  await dialog(page).getByRole('button',{name:'Apply to 1 ready row',exact:true}).click();
  await expect(dialog(page).getByText('Succeeded',{exact:true})).toHaveCount(1);
  const affected = dialog(page).getByRole('list',{name:'What this applies to'});
  await expect(affected.locator('li',{hasText:'Upgrade deps'}).getByRole('alert')).toBeVisible();
  const changes = await sent();
  expect(changes.map((change) => change.id)).toEqual([ACME.fixLogin]);
});

test('a lost share answer is read explicitly and the one Undo never treats that read as a receipt', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}?view=list`);
  await page.getByRole('checkbox',{name:'Select Fix login',exact:true}).check();
  await page.getByRole('checkbox',{name:'Select Upgrade deps',exact:true}).check();
  const written: string[] = [];
  await page.route('**/api/v1/nodes/*/permissions', async (route) => {
    if (route.request().method() !== 'PATCH') { await route.continue(); return; }
    const id = route.request().url().split('/').at(-2) ?? '';
    written.push(id);
    if (id === ACME.upgradeDeps) { await route.fetch(); await route.abort('connectionreset'); }
    else await route.continue();
  });
  await page.getByRole('region',{name:'Selected topics'}).getByRole('button',{name:'Share…',exact:true}).click();
  await stage(page,'Vera Rubin');
  await dialog(page).getByRole('checkbox',{name:/Send Vera Rubin a message with the link/}).uncheck();
  await dialog(page).getByRole('button',{name:'Review shares',exact:true}).click();
  expect(written).toEqual([]);
  await dialog(page).getByRole('button',{name:'Apply to 2 ready rows',exact:true}).click();
  await expect(dialog(page).getByText('Answer lost; outcome unknown',{exact:true})).toBeVisible();
  const lost = dialog(page).getByRole('list',{name:'What this applies to'}).getByRole('listitem').filter({has:page.getByText('Upgrade deps',{exact:true})});
  await lost.getByRole('button',{name:'Read current state',exact:true}).click();
  await expect(lost).toContainText('This read is not a confirmation of the lost write.');
  await dialog(page).getByRole('button',{name:'Undo successful rows',exact:true}).click();
  await expect(dialog(page).getByRole('heading',{name:'Undo results',exact:true})).toBeVisible();
  expect(written).toEqual([ACME.fixLogin,ACME.upgradeDeps,ACME.fixLogin]);
});
