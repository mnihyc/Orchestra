/** Third-pass navigation: one current destination, topic exits, canonical addresses and one Actions sign. */
import { expect, type Page, test } from '@playwright/test';
import { expectOneCurrentSection, navigationFaults } from '../accessibility/navigation.ts';
import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: 'Language', exact: true }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}
async function left(page: Page, width: number) {
  await expect(page.locator('[data-workbench-chrome]')).toBeAttached();
  const panel = page.locator('[data-workbench-bar="left"]:visible');
  if (await panel.count() === 0) {
    if (width === 400) await page.locator('[data-container-choice="left:navigation"]').click();
    else await page.getByRole('button', { name: /^(Left bar|左栏)$/, exact: true }).click();
  }
  await expect(panel).toBeVisible(); return panel;
}
async function actions(page: Page, width: number) {
  const panel = await left(page, width);
  const title = panel.locator('[data-container-title]');
  const menu = title.locator('button[data-action="actions"][aria-haspopup="menu"]');
  await expect(menu).toHaveCount(1); await expect(menu.locator('svg.lucide-ellipsis')).toHaveCount(1);
  await expect(menu.locator('svg circle')).toHaveCount(3);
  await expect(title.locator('button[data-action="settings"],button[data-action="help"]')).toHaveCount(0);
}

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`current sections and header actions agree with the visible screen at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    for (const [path, current] of [['/usage', 'usage'], ['/usage?view=performance', 'performance'], ['/usage?view=requests', 'requests'], ['/settings/interface', 'interface'], ['/help?part=articles', 'articles'], ['/admin', 'access']] as const) {
      await page.goto(path); const panel = await left(page, width);
      const nav = panel.locator('[data-section-nav]'); await expect(nav).toHaveCount(1);
      await expect(nav.locator('[data-section-id][aria-current="page"]')).toHaveCount(1);
      await expect(nav.locator('[aria-current="page"]')).toHaveAttribute('data-section-id', current);
      await expectOneCurrentSection(page); await actions(page, width);
      if (path === '/admin') {
        await expect(nav.getByRole('link')).toHaveCount(1);
        await expect(nav.locator('[data-section-id="overview"]')).toHaveCount(0);
      }
    }
    if (width === 400) await page.locator('[data-container-choice="left:navigation"]').click();
    await page.getByRole('button', { name: /^(Turn on As admin|启用管理员模式)$/, exact: true }).press('Enter');
    const panel = await left(page, width);
    await expect(panel.locator('[data-section-id="overview"]')).toHaveAttribute('aria-current', 'page');
    await expectOneCurrentSection(page);
  });

  test(`a topic's own link and breadcrumb leave Settings at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale); await page.goto(`/topic/${ACME.fixLogin}`);
    const settings = locale === 'en' ? 'Settings' : '设置';
    const overview = locale === 'en' ? 'Overview' : '概览';
    const activity = locale === 'en' ? 'Activity' : '动态';
    const tabs = () => page.getByRole('tablist').filter({ has: page.getByRole('tab', { name: overview, exact: true }) });
    await tabs().getByRole('tab', { name: activity, exact: true }).click();
    await tabs().getByRole('tab', { name: settings, exact: true }).click();
    await expect(page).toHaveURL(/tab=settings/);
    const stored = await page.evaluate(() => Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)).flatMap((key): string[] => key?.endsWith('.choices.topic.tab') ? [key] : []));
    expect(stored).toHaveLength(1);
    const preference = stored[0]; if (preference === undefined) throw new Error('The topic content preference is missing');
    expect(await page.evaluate((key) => localStorage.getItem(key), preference)).toBe('"activity"');
    const parent = page.locator('[data-screen-path]').getByRole('link', { name: 'Fix login', exact: true });
    await parent.press('Enter'); await expect(tabs()).toBeVisible();
    await expect(tabs().getByRole('tab', { name: overview, exact: true })).toHaveAttribute('aria-selected', 'true');
    // A legacy installation may still have stored Settings. Following the actual topic link must escape it too.
    await page.evaluate((key) => localStorage.setItem(key, '"settings"'), preference);
    await page.goto(`/project/${ACME.web}`);
    await page.getByRole('link', { name: 'Fix login', exact: true }).first().click();
    await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}/?(?:\\?|$)`));
    await expect(tabs()).toBeVisible();
    await expect(tabs().getByRole('tab', { name: overview, exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('[data-settings-frame]')).toHaveCount(0);
  });

  test(`org and department settings accept identifier addresses at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const reads: string[] = [];
    page.on('request', (request) => { if (request.method() === 'GET') reads.push(new URL(request.url()).pathname); });
    for (const [id, address] of [[ACME.org, 'acme'], [ACME.research, 'acme.research']] as const) {
      reads.length = 0;
      await page.goto(`/org/${id}/settings/members?setting=model.default#invitation`);
      await expect(page).toHaveURL(new RegExp(`/org/${address.replaceAll('.', '\\.')}/settings/members\\?setting=model\\.default#invitation$`));
      await expect(page.locator('[data-settings-frame]')).toBeVisible();
      const panel = await left(page, width);
      await expect(panel.locator('[data-section-id="members"]')).toHaveAttribute('aria-current', 'page');
      await expectOneCurrentSection(page); await actions(page, width);
      expect(reads.filter((path) => path === `/api/v1/orgs/by-slug/${id}`)).toEqual([]);
      // A bootstrap-cached org needs no lookup; otherwise the loader/sidebar share one native read.
      expect(reads.filter((path) => path === `/api/v1/nodes/${id}`).length).toBeLessThanOrEqual(1);
    }
  });
}

test('the layout audit rejects two, absent or wrong current destinations and ignores a covered list', async ({ page }) => {
  // This checks the auditor itself against static DOM; the scenarios above exercise real routed screens.
  await page.setContent('<nav aria-label="Sections" data-section-nav data-current-section="usage"><a href="/usage" data-section-id="usage" aria-current="page">Usage</a><a href="/usage?view=performance" data-section-id="performance" aria-current="page">Performance</a></nav>');
  expect(await navigationFaults(page)).toHaveLength(1);
  await page.locator('[data-section-id="performance"]').evaluate((element) => element.removeAttribute('aria-current'));
  await expectOneCurrentSection(page);
  await page.locator('[data-section-id="usage"]').evaluate((element) => element.removeAttribute('aria-current'));
  expect(await navigationFaults(page)).toHaveLength(1);
  await page.locator('[data-section-id="performance"]').evaluate((element) => element.setAttribute('aria-current', 'page'));
  expect(await navigationFaults(page)).toHaveLength(1);
  await page.locator('nav').evaluate((element) => element.setAttribute('aria-hidden', 'true'));
  await expectOneCurrentSection(page);
});
