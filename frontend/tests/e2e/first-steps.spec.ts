/** A team owner's setup choice, scoped facts and portable dismissal; personal spaces have no team-setup card. */
import { expect, test } from '@playwright/test';

import { ACME, deny, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const CONTROL_TIMEOUT_MS = 10_000;
async function control(path: string, body: Record<string, unknown> = {}) {
  const response = await fetch(`${DOUBLE}/__double/first-steps${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(CONTROL_TIMEOUT_MS) });
  expect(response.ok).toBe(true);
  return response.status === 204 ? null : response.json() as Promise<Record<string, unknown>>;
}

test.beforeEach(async () => { await scenario(); });

async function setupTeam() {
  const created = await control('/org', { name: 'Setup team', slug: 'setup-team' });
  const id = created?.id;
  if (typeof id !== 'string') throw new Error('the fixture must create a root org');
  return id;
}

test('a fresh administrator’s own space starts with its project action and no team-setup card', async ({ page }) => {
  await scenario({ scenario: 'fresh' });
  await page.goto('/setup');
  await page.getByLabel('Username', { exact: true }).fill('firstperson');
  await page.getByLabel('Name', { exact: true }).fill('First Person');
  await page.getByLabel('Password', { exact: true }).fill('a first safe phrase');
  await page.getByRole('button', { name: 'Create administrator', exact: true }).click();
  await page.waitForURL(/\/org\/firstperson$/);
  const first = page.getByTestId('first-steps');
  await expect(first).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'New project', exact: true })).toHaveCount(1);
  await page.getByRole('button', { name: 'New project', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'New project in First Person' })).toBeVisible();
});

test('many readable teams keep personal fallback without setup guidance and cancelling creation leaves the place alone', async ({ page }) => {
  await control('/org', { name: 'Another team', slug: 'another-team' });
  await control('/landing', { org: '00000000-0000-4000-8000-ffffffffffff' });
  await signIn(page);
  await expect(page).toHaveURL(/\/org\/ada$/);
  const first = page.getByTestId('first-steps');
  await expect(first).toHaveCount(0);
  const create = page.getByTestId('main').getByRole('button', { name: 'New project', exact: true });
  await create.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog', { name: 'New project in Ada Lovelace' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(page).toHaveURL(/\/org\/ada$/);
  await expect(first).toHaveCount(0);
});

test('a readable saved department remains the landing place without changing the root navigation', async ({ page }) => {
  const reads: string[] = [];
  page.on('request', (request) => { if (request.url().includes('/me/first-steps')) reads.push(request.url()); });
  await control('/landing', { org: ACME.research });
  await control('/choice', { org: ACME.research, mode: 'team' });
  await signIn(page);
  await expect(page).toHaveURL(/\/org\/acme\.research$/);
  await expect(page.getByRole('heading', { name: 'Research', exact: true })).toBeVisible();
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
  expect(reads).toEqual([]);
});

test('an established root has no setup card even before a mode was chosen', async ({ page }) => {
  const read = page.waitForResponse((response) => response.url().includes('/me/first-steps') && response.url().includes(ACME.org));
  await signIn(page);
  await read;
  await expect(page.getByRole('heading', { name: 'Acme', exact: true })).toBeVisible();
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
});

test('a member never requests or receives the owner’s setup card', async ({ page }) => {
  const reads: string[] = [];
  page.on('request', (request) => { if (request.url().includes('/me/first-steps')) reads.push(request.url()); });
  await signIn(page, PEOPLE.vera);
  await page.goto('/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme', exact: true })).toBeVisible();
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
  expect(reads).toEqual([]);
});

test('an unfinished root offers setup without repeating its name or probing a model', async ({ page }) => {
  await setupTeam();
  const acts: string[] = [];
  page.on('request', (request) => { if (request.method() !== 'GET' && /\/providers|\/threads\/.*inputs/.test(request.url())) acts.push(request.url()); });
  await signIn(page);
  await page.goto('/org/setup-team');
  const first = page.getByTestId('first-steps');
  await expect(first.getByRole('button', { name: 'Use this team', exact: true })).toBeVisible();
  await expect(first.getByRole('link', { name: 'Open provider' })).toHaveCount(0);
  await first.getByRole('button', { name: 'Use this team', exact: true }).click();
  await expect(first.getByText('Setup team', { exact: true })).toHaveCount(0);
  await expect(first.locator('[data-step="team"]')).toHaveCount(0);
  await expect(first.locator('[data-step="provider"]')).toContainText('Not done yet');
  await expect(first.locator('[data-step="work"]')).toContainText('Not done yet');
  await expect(first.getByRole('button', { name: 'Refresh', exact: true })).toHaveCount(0);
  await expect(first.getByText(/Checked|Last checked/)).toHaveCount(0);
  expect(acts).toEqual([]);
});

test('an incomplete team keeps its own facts and remembers dismissal on another window and device', async ({ page, browser }) => {
  const team = await setupTeam();
  await control('/choice', { org: team, mode: 'team' });
  await signIn(page);
  await page.goto('/org/setup-team');
  const first = page.getByTestId('first-steps');
  await expect(first.locator('[data-step="provider"]')).toContainText('Not done yet');
  await expect(first.locator('[data-step="teammate"]')).toContainText('Not done yet');
  const peer = await page.context().newPage();
  try {
    await peer.goto('/org/setup-team');
    await expect(peer.getByRole('button', { name: 'Dismiss', exact: true })).toBeVisible();
    const reads: string[] = [];
    peer.on('request', (request) => { if (request.method() === 'GET' && /\/me\/(first-steps|state)(?:\?|$)/.test(request.url())) reads.push(request.url()); });
    const writes: string[] = [];
    page.on('request', (request) => { if (request.method() !== 'GET' && request.url().includes('/me/first-steps')) writes.push(request.method()); });
    await first.getByRole('button', { name: 'Dismiss', exact: true }).click();
    await expect(peer.getByRole('button', { name: 'Show first steps' })).toBeVisible();
    expect(reads).toEqual([]);
    expect(writes).toEqual(['PUT']);
  } finally { await peer.close(); }
  await expect(first.getByRole('button', { name: 'Show first steps' })).toBeVisible();
  const saved = await control('/state');
  expect(saved?.state).toMatchObject({ sidebar: { last_org_id: team }, help: { first_steps: { org_id: team, work_mode: 'team', dismissed: true } } });
  const other = await browser.newContext();
  try {
    const second = await other.newPage();
    await signIn(second);
    await expect(second).toHaveURL(/\/org\/setup-team$/);
    await expect(second.getByRole('button', { name: 'Show first steps' })).toBeVisible();
    await second.getByRole('button', { name: 'Show first steps' }).click();
    await expect(second.getByTestId('first-steps').locator('[data-step="provider"]')).toContainText('Not done yet');
    await expect(page).toHaveURL(/\/org\/setup-team$/);
  } finally { await other.close(); }
});

test('a new team is created in the existing form before its first provider or invitation', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/ada');
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
  await page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: Off', exact: true }).click();
  await page.getByRole('button', { name: 'New org…', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('First steps team');
  await dialog.getByRole('textbox', { name: 'Short name', exact: true }).fill('first-steps-team');
  await dialog.getByRole('button', { name: 'Create org', exact: true }).click();
  await expect(page).toHaveURL(/\/org\/first-steps-team$/);
  const first = page.getByTestId('first-steps');
  await first.getByRole('button', { name: 'Use this team', exact: true }).click();
  await expect(first.locator('[data-step="provider"]')).toContainText('Not done yet');
  await expect(first.locator('[data-step="teammate"]')).toContainText('Not done yet');
  await first.getByRole('link', { name: 'Invite teammate' }).click();
  await expect(page).toHaveURL(/tab=members/);
  await page.getByRole('button', { name: 'Invite…', exact: true }).click();
  await page.getByRole('dialog').getByRole('radio', { name: 'Link' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Create invitation' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Done', exact: true }).click();
  await page.goto('/org/first-steps-team');
  await expect(page.getByRole('tab', { name: /^Members/ })).toHaveAttribute('aria-selected', 'true');
  await page.getByRole('tab', { name: 'Overview', exact: true }).click();
  await expect(page.getByTestId('first-steps').getByText('Waiting for acceptance')).toBeVisible();
});

test('loading or failed observations do not invent missing setup, and lost View exposes no old facts', async ({ page }) => {
  const org = await setupTeam();
  await control('/choice', { org, mode: 'team' });
  await control('/set', { delayMs: 1800 });
  await signIn(page);
  await page.goto('/org/setup-team');
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
  await expect(page.getByTestId('first-steps').locator('[data-step="provider"]')).toContainText('Not done yet');
  await control('/set', { delayMs: 0, failed: true });
  const failedRead = page.waitForResponse((response) => response.url().includes('/me/first-steps') && response.status() >= 400);
  await page.reload();
  await failedRead;
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
  await control('/set', { failed: false });
  await deny('ada', [`org.view@${org}`]);
  await page.reload();
  await expect(page.getByTestId('first-steps')).toHaveCount(0);
});

test('the current readable Start here title opens its normal document and disappears when deleted', async ({ page }) => {
  const org = await setupTeam();
  const created = await control('/document', { org, title: 'Team welcome' });
  const id = String(created?.id);
  await signIn(page);
  await page.goto('/org/setup-team');
  const guide = page.getByRole('region', { name: 'Start here' });
  await expect(guide.getByText('Team welcome', { exact: true })).toBeVisible();
  await guide.getByRole('link', { name: /Open .*Team welcome/ }).click();
  await expect(page).toHaveURL(new RegExp(`/doc/${id}$`));
  await expect(page.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue('Team welcome');
  await control('/document-change', { id, title: 'Our first day' });
  await page.goto('/org/setup-team');
  await expect(guide.getByText('Our first day', { exact: true })).toBeVisible();
  await control('/document-change', { id, hidden: true });
  await page.reload();
  await expect(guide).toHaveCount(0);
  await expect(page.getByText('Our first day', { exact: true })).toHaveCount(0);
  await control('/document-change', { id, deleted: true });
  await page.reload();
  await expect(guide).toHaveCount(0);
});

for (const [locale, useTeam] of [['en', 'Use this team'], ['zh-CN', '使用此团队']] as const) {
test(`the compact card works by keyboard at 1440 and 400 in both themes (${locale})`, async ({ page }) => {
  await setupTeam();
  await control('/appearance', { locale });
  await signIn(page);
  await page.goto('/org/setup-team');
  const first = page.getByTestId('first-steps');
  const use = first.getByRole('button', { name: useTeam, exact: true });
  await expect(use).toBeVisible();
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    for (const action of await first.locator('button, a').all()) {
      const heights = await action.evaluate((element) => {
        const probe = document.createElement('span');
        probe.style.cssText = 'position:absolute;visibility:hidden;display:block;width:0;height:var(--control-sm)';
        element.parentElement?.append(probe);
        const heights = { actual: element.getBoundingClientRect().height, compact: probe.getBoundingClientRect().height };
        probe.remove();
        return heights;
      });
      expect(Math.abs(heights.actual - heights.compact)).toBeLessThan(1);
    }
    const widths = await use.evaluate((element) => ({ button: element.getBoundingClientRect().width,
      row: element.parentElement?.getBoundingClientRect().width ?? 0 }));
    expect(widths.button).toBeLessThan(widths.row);
    await expectAccessibleInBothThemes(page, '[data-testid="first-steps"]');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
  await use.focus();
  await page.keyboard.press('Enter');
  await expect(first.locator('[data-step="provider"]')).toBeVisible();
  await expect(first.locator('[data-step="team"]')).toHaveCount(0);
  await page.goto('/org/ada');
  await expect(first).toHaveCount(0);
  await expect(page.getByTestId('main').getByRole('button', { name: locale === 'en' ? 'New project' : '新建项目', exact: true })).toHaveCount(1);
});
}

test('a chosen progress fold survives another org, reload and a second window', async ({ page }) => {
  const org = await setupTeam();
  await control('/choice', { org, mode: 'team' });
  await signIn(page);
  await page.goto('/org/setup-team');
  const progress = page.getByTestId('first-steps').locator('details');
  await expect(progress).toHaveAttribute('open', '');
  await progress.locator('summary').click();
  await expect(progress).not.toHaveAttribute('open');
  await page.goto('/org/ada');
  await expect(page.getByTestId('first-steps').locator('details')).toHaveCount(0);
  await page.goto('/org/setup-team');
  await page.reload();
  await expect(progress).not.toHaveAttribute('open');
  const second = await page.context().newPage();
  try {
    await second.goto('/org/setup-team');
    await expect(second.getByTestId('first-steps').locator('details')).not.toHaveAttribute('open');
    await expect(progress).not.toHaveAttribute('open');
  } finally { await second.close(); }
});
