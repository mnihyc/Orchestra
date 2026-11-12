/** R3 W8: reach Usage, ask about last week in an org, and find a thread without a hidden tab. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';
import { ACME, DAY, writeHistory } from './usage-support';

test.beforeEach(async()=>{await scenario();});

test('the account menu leads to last week for a place and a visible thread breakdown',async({page})=>{
  await signIn(page);
  await page.getByRole('button',{name:'Your account'}).click();
  await page.getByRole('menuitem',{name:'Usage',exact:true}).click();
  await expect(page).toHaveURL(/\/usage/);
  // Report controls live in the filter bar and adapt to the report pane's width.
  const filters=page.locator('[data-filter-bar]');
  const scope=filters.getByRole('combobox',{name:'Scope'});
  await scope.click();
  // The shared name locator searches places on demand rather than preloading all orgs.
  await scope.fill('Acme');
  await page.getByRole('option',{name:'Acme',exact:true}).click();
  const week=filters.getByRole('button',{name:'Last week',exact:true});
  if(await week.isVisible()) await week.click();
  else {
    await filters.getByRole('combobox',{name:'Span',exact:true}).click();
    await page.getByRole('option',{name:'Last week',exact:true}).click();
  }
  await expect.poll(()=>new URL(page.url()).searchParams.get('scope')).toBe(`node:${ACME.org}`);
  await expect.poll(()=>new URL(page.url()).searchParams.get('range')).toBe('last_week');
  const totals=page.getByRole('list',{name:'Figures of the span'});
  await expect(totals.getByRole('listitem').filter({hasText:'Turns'})).toContainText(String(DAY.all.turns*7));
  const byThread=page.getByRole('tab',{name:'By thread',exact:true});
  await expect(byThread).toBeVisible();await byThread.click();
  await expect(page.getByRole('table',{name:'By thread',exact:true})).toBeVisible();
  await expect(page.getByRole('table',{name:'Records',exact:true})).toBeVisible();
  await expect(page.getByText('Days use UTC',{exact:true})).toHaveCount(1);
});

test('an empty place’s usage card still opens its scoped report',async({page})=>{
  await writeHistory(0);await signIn(page);await page.goto('/org/acme');
  const board=page.getByRole('region',{name:'Usage',exact:true});
  const open=board.getByRole('link',{name:'Open usage',exact:true});
  await expect(open).toBeVisible();await open.click();
  await expect.poll(()=>new URL(page.url()).searchParams.get('scope')).toBe(`node:${ACME.org}`);
  await expect(page.getByRole('combobox',{name:'Scope'})).toHaveValue('Acme');
});
