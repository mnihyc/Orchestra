/** Compact headers, honest department people and personal-space actions in both languages, widths and themes. */
import { expect, test } from '@playwright/test';
import { expectAccessible, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const CONTROL_TIMEOUT_MS = 10_000;
test.beforeEach(async () => { await scenario(); });

for (const locale of ['en', 'zh-CN'] as const) {
  test(`each org kind keeps only star, primary and secondary menu (${locale})`, async ({ page }) => {
    await signIn(page);
    const response = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ locale, theme: 'system' }), signal: AbortSignal.timeout(CONTROL_TIMEOUT_MS) });
    expect(response.ok).toBe(true);
    let reads: string[] = [];
    let setupWrites: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET') reads.push(path);
      else if (path.endsWith('/me/first-steps')) setupWrites.push(path);
    });
    for (const [slug, name] of [['acme', 'Acme'], ['acme.research', 'Research'], ['ada', 'Ada Lovelace']] as const) {
      reads = []; setupWrites = [];
      await page.goto(`/org/${slug}`);
      const main = page.locator('[data-main-area]');
      const title = main.getByRole('heading', { name, level: 1, exact: true });
      await expect(title).toBeVisible();
      const header = main.locator('header').filter({ has: page.getByRole('heading', { name, level: 1, exact: true }) });
      await expect(header.getByRole('button')).toHaveCount(3);
      await expect(header.locator('button[aria-pressed]')).toHaveCount(1);
      const menu = header.locator('button[aria-haspopup="menu"]');
      await expect(menu).toHaveCount(1);
      if (slug === 'ada') {
        await expect(page.getByTestId('first-steps')).toHaveCount(0);
        expect(reads.filter((path) => path.endsWith('/me/first-steps'))).toEqual([]);
        expect(setupWrites).toEqual([]);
      }
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: 900 });
        for (const theme of ['light', 'dark'] as const) {
          await expectAccessible(page, theme, '[data-main-area]');
          const geometry = await header.evaluate((element) => {
            const box = element.getBoundingClientRect();
            return { fits: box.right <= window.innerWidth, controls: Array.from(element.querySelectorAll('button')).map((button) => {
              const control = button.getBoundingClientRect(); const icon = button.querySelector('svg')?.getBoundingClientRect();
              return { height: control.height, offset: icon === undefined ? 0 : Math.abs(icon.top + icon.height / 2 - control.top - control.height / 2) };
            }) };
          });
          expect(geometry.fits).toBe(true);
          for (const control of geometry.controls) { expect(control.height).toBeLessThanOrEqual(32); expect(control.offset).toBeLessThan(1); }
          await page.screenshot({ path: test.info().outputPath(`org-header-${slug}-${locale}-${width}-${theme}.png`), fullPage: true });
        }
      }
      await menu.focus(); await page.keyboard.press('Enter');
      const actions = page.getByRole('menu');
      const settings = actions.getByRole('menuitem', { name: locale === 'en' ? 'Settings' : '设置', exact: true });
      await expect(settings).toHaveAttribute('href', `/org/${slug}/settings/general`);
      if (slug === 'ada') await expect(actions.getByRole('menuitem', { name: /New department|新建部门/ })).toHaveCount(0);
      await page.keyboard.press('Escape'); await expect(menu).toBeFocused();
      await menu.click(); await settings.click();
      await expect(page).toHaveURL(new RegExp(`/org/${slug.replaceAll('.', '\\.')}/settings/general`));
    }
  });

  test(`a department with inherited people never claims its domain zero is the people count (${locale})`, async ({ page }) => {
    await signIn(page);
    const response = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ locale, theme: 'system' }), signal: AbortSignal.timeout(CONTROL_TIMEOUT_MS) });
    expect(response.ok).toBe(true);
    await page.goto('/org/acme');
    const department = page.locator('[data-main-area] [data-places-list] article').filter({ has: page.getByRole('link', { name: 'Research', exact: true }) });
    await expect(department).toBeVisible();
    await expect(department).not.toContainText('acme.research');
    await expect(department.getByText(locale === 'en' ? /^[1-9]\d* (person|people)$/ : /^[1-9]\d* 人$/)).toBeVisible();
    await expect(department.getByText(locale === 'en' ? /^0 people$/ : /^0 人$/)).toHaveCount(0);
  });
}
