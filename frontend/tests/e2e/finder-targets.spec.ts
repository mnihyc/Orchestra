/** Finder results reach the actual permission row, person share and setting key, including a second key on the same screen. */
import { expect, type Page, test } from '@playwright/test';
import { id } from '../doubles/fixtures.ts';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const finder = (page: Page) => page.getByRole('dialog',{name:'Find',exact:true});
async function find(page: Page, text: string) {
  await page.keyboard.press('Control+k');
  await expect(finder(page)).toBeVisible();
  await finder(page).getByRole('combobox',{name:'Search terms',exact:true}).fill(text);
}
test.beforeEach(async () => { await scenario(); });

test('a permission result discloses its real row and receives focus after the finder leaves', async ({page}) => {
  await signIn(page,PEOPLE.ada);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button',{name:'Actions for Fix login'}).first().click();
  await page.getByRole('menuitem',{name:'Share…'}).click();
  const access = page.getByRole('dialog',{name:'Access to Fix login',exact:true});
  await expect(access).toBeVisible();
  await find(page,': topic.archive');
  const result = finder(page).getByRole('option').filter({hasText:'Archive Topic'});
  await expect(result).toBeVisible();
  await result.click();
  const row = access.locator('[data-access-permission="topic.archive"]');
  await expect(finder(page)).toHaveCount(0);
  await expect(row).toBeVisible();
  await expect(row).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(access).toHaveCount(0);
  await expect(page.getByRole('button',{name:'Actions for Fix login'}).first()).toBeFocused();
});

test('a person card shows Message and a current share which opens its existing Access dialog', async ({page}) => {
  const given = await fetch(`${DOUBLE}/__double/access/touch`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({as:'ada',path:'nodes',id:ACME.fixLogin,change:{name:[{choice:'full_view',holders:[{kind:'user',id:id(1,9)}]}]}})});
  expect(given.ok).toBeTruthy();
  await signIn(page,PEOPLE.ada);
  await find(page,'@dora');
  await finder(page).getByRole('option').filter({hasText:'Dora Developer'}).click();
  await expect(finder(page).getByRole('button',{name:'Message',exact:true})).toBeVisible();
  const shares = finder(page).getByRole('region',{name:'Direct shares between you'});
  await shares.getByRole('button',{name:'Fix login',exact:true}).click();
  const access = page.getByRole('dialog',{name:'Access to Fix login',exact:true});
  await expect(finder(page)).toHaveCount(0);
  await expect(access).toBeVisible();
  await expect(access.getByRole('group',{name:'Can view',exact:true})).toContainText('Dora Developer');
});

test('a setting key survives routing and a second key focuses another row on the same editor', async ({page}) => {
  await signIn(page,PEOPLE.ada);
  await page.goto('/org/acme');
  await find(page,': agent.instructions');
  await finder(page).getByRole('option').filter({hasText:'Instructions'}).click();
  await expect(page).toHaveURL(/setting=agent.instructions/);
  const first = page.locator('[data-setting="agent.instructions"]');
  await expect(first).toBeVisible();
  await expect(first).toBeFocused();
  await find(page,': approval.mode');
  await finder(page).getByRole('option').filter({hasText:'Approval mode'}).click();
  await expect(page).toHaveURL(/setting=approval.mode/);
  const second = page.locator('[data-setting="approval.mode"]');
  await expect(second).toBeVisible();
  await expect(second).toBeFocused();
});
